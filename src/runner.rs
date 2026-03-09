use futures::future::join_all;
use std::collections::HashMap;
use std::sync::{Arc, RwLock, Mutex};
use tokio::time::{Duration, Instant, interval};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::common::{RoundResults, TestrpcError};
use crate::config::{self, AdapterConfig, LoadStage};
use crate::{adapters, ctx};

/// Shared state for current load stage
#[derive(Debug, Clone)]
pub struct LoadStageInfo {
    pub stage_index: usize,
    pub target_tx_per_second: u64,
    pub stage_start: Instant,
    pub stage_duration: Duration,
}

/// Per-node statistics for continuous streaming
#[derive(Debug, Clone, Default)]
pub struct NodeStats {
    pub sent: u64,
    pub failed: u64,
    pub bytes_sent: u64,
    pub last_reported: Option<Instant>,
}

pub async fn load_endpoints(cfg: config::Config) -> Result<Vec<String>, TestrpcError> {
    if let Some(rpcs) = cfg.rpcs {
        return Ok(rpcs);
    }
    let adapter = adapters::new_adapter(cfg.adapter)?;
    adapter
        .load_endpoints(cfg.args.clone())
        .await
        .map_err(|e| TestrpcError::LoadEndpointsError(e.to_string()))
}

/// Run the test flow with the given configuration.
/// This function will run the test flow until we reach cfg.iterations or if the context is stopped.
/// Upon completion, we wait for all the open threads to complete. and the function will return a vector of RoundResults.
pub async fn run(
    ctx: Arc<ctx::Context>,
    cfg: config::Config,
    rpc_urls: Vec<String>,
) -> Result<Vec<RoundResults>, TestrpcError> {
    // Check if we should use continuous streaming mode or legacy batch mode
    // Continuous mode is enabled by either load_stages OR (duration_seconds AND target_tx_per_second)
    let use_continuous_mode = cfg.load_stages.is_some() 
        || (cfg.duration_seconds.is_some() && cfg.target_tx_per_second.is_some());
    
    if use_continuous_mode {
        run_continuous_mode(ctx, cfg, rpc_urls).await
    } else {
        run_legacy_batch_mode(ctx, cfg, rpc_urls).await
    }
}

/// NEW: Continuous streaming mode with persistent connections
async fn run_continuous_mode(
    ctx: Arc<ctx::Context>,
    cfg: config::Config,
    rpc_urls: Vec<String>,
) -> Result<Vec<RoundResults>, TestrpcError> {
    // Check if using load stages (variable rate) or single rate
    let load_stages = if let Some(stages) = cfg.load_stages.clone() {
        if stages.is_empty() {
            return Err(TestrpcError::LoadConfigError(
                "load_stages cannot be empty".to_string(),
                "".to_string(),
            ));
        }
        stages
    } else if let (Some(duration), Some(target_rate)) = (cfg.duration_seconds, cfg.target_tx_per_second) {
        // Convert single rate config to a single-stage format for unified handling
        vec![LoadStage {
            duration_seconds: duration,
            target_tx_per_second: target_rate,
        }]
    } else {
        return Err(TestrpcError::LoadConfigError(
            "Continuous mode requires either 'load_stages' or both 'duration_seconds' and 'target_tx_per_second'".to_string(),
            "".to_string(),
        ));
    };
    
    let total_duration: u64 = load_stages.iter().map(|s| s.duration_seconds).sum();
    let is_variable_load = load_stages.len() > 1;
    
    tracing::info!("Runner starting in CONTINUOUS MODE{}:", if is_variable_load { " (VARIABLE LOAD)" } else { "" });
    tracing::info!("  Total duration: {} seconds", total_duration);
    tracing::info!("  Nodes: {}", rpc_urls.len());
    
    if is_variable_load {
        tracing::info!("  Load stages:");
        for (i, stage) in load_stages.iter().enumerate() {
            tracing::info!("    Stage {}: {} tx/s per node for {} seconds (aggregate: {} tx/s)", 
                i + 1, 
                stage.target_tx_per_second, 
                stage.duration_seconds,
                stage.target_tx_per_second * rpc_urls.len() as u64);
        }
    } else {
        tracing::info!("  Target: {} tx/s per node", load_stages[0].target_tx_per_second);
        tracing::info!("  Total target throughput: {} tx/s", load_stages[0].target_tx_per_second * rpc_urls.len() as u64);
    }
    
    // Get transaction template
    let template = cfg.rounds.first()
        .and_then(|r| r.get_template(cfg.round_templates.clone()))
        .ok_or_else(|| TestrpcError::LoadRoundTemplateError("No template found".to_string()))?;
    
    let tx_size = template.tx_size;
    
    // Check for dry-run mode
    let is_dry_run = std::env::var("DRY_RUN").is_ok();
    if is_dry_run {
        tracing::info!("DRY_RUN mode: Would establish persistent connections to {} nodes", rpc_urls.len());
        if is_variable_load {
            tracing::info!("DRY_RUN mode: Would execute {} load stages over {} seconds", load_stages.len(), total_duration);
            for (i, stage) in load_stages.iter().enumerate() {
                tracing::info!("DRY_RUN mode:   Stage {}: {} tx/s per node for {} seconds", 
                    i + 1, stage.target_tx_per_second, stage.duration_seconds);
            }
        } else {
            tracing::info!("DRY_RUN mode: Would send {} tx/s per node for {} seconds", 
                load_stages[0].target_tx_per_second, total_duration);
        }
        
        let total_tx: u64 = load_stages.iter()
            .map(|s| s.target_tx_per_second * s.duration_seconds)
            .sum();
        tracing::info!("DRY_RUN mode: Total expected transactions: {} per node, {} aggregate",
            total_tx, total_tx * rpc_urls.len() as u64);
        
        return Ok(vec![RoundResults {
            sent: (total_tx * rpc_urls.len() as u64) as usize,
            failed: 0,
        }]);
    }
    
    // Establish persistent connections to all nodes (tolerates failures —
    // unreachable nodes will be retried by the sender task's reconnect logic)
    tracing::info!("Establishing persistent connections to {} nodes...", rpc_urls.len());
    let mut connections: Vec<(usize, String, Option<Framed<TcpStream, LengthDelimitedCodec>>)> = Vec::new();
    let mut connected_count = 0usize;

    for (idx, url) in rpc_urls.iter().enumerate() {
        match tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(url)).await {
            Ok(Ok(stream)) => {
                if let Err(e) = stream.set_nodelay(true) {
                    tracing::warn!("Failed to set TCP_NODELAY for connection to node {}: {}", idx, e);
                }
                let framed = Framed::new(stream, LengthDelimitedCodec::new());
                connections.push((idx, url.clone(), Some(framed)));
                connected_count += 1;
                tracing::debug!("Connected to node {} ({})", idx, url);
            }
            Ok(Err(e)) => {
                tracing::warn!("Failed to connect to node {} ({}): {} — will retry later", idx, url, e);
                connections.push((idx, url.clone(), None));
            }
            Err(_) => {
                tracing::warn!("Connection to node {} ({}) timed out — will retry later", idx, url);
                connections.push((idx, url.clone(), None));
            }
        }
    }

    if connected_count == 0 {
        return Err(TestrpcError::RpcError("Failed to connect to ANY node".to_string()));
    }

    tracing::info!("Established {}/{} initial connections ({} will retry in background)",
        connected_count, rpc_urls.len(), rpc_urls.len() - connected_count);
    
    // Shared statistics across all sender tasks
    let stats = Arc::new(Mutex::new(HashMap::<usize, NodeStats>::new()));
    for (idx, _, _) in &connections {
        stats.lock().unwrap().insert(*idx, NodeStats::default());
    }

    
    // Shared current load stage information
    let test_start = Instant::now();
    let current_stage = Arc::new(Mutex::new(LoadStageInfo {
        stage_index: 0,
        target_tx_per_second: load_stages[0].target_tx_per_second,
        stage_start: test_start,
        stage_duration: Duration::from_secs(load_stages[0].duration_seconds),
    }));
    
    // Spawn sender tasks for each node
    let mut sender_handles = Vec::new();
    let test_duration = Duration::from_secs(total_duration);

    for (node_idx, node_url, maybe_transport) in connections {
        let stats_clone = Arc::clone(&stats);
        let current_stage_clone = Arc::clone(&current_stage);
        let ctx_clone = Arc::clone(&ctx);
        let quit_rx = ctx_clone.recv();
        let initially_connected = maybe_transport.is_some();

        let handle = tokio::spawn(async move {
            continuous_sender_task(
                node_idx,
                node_url,
                maybe_transport,
                initially_connected,
                tx_size,
                stats_clone,
                current_stage_clone,
                test_duration,
                test_start,
                quit_rx,
            ).await
        });

        sender_handles.push(handle);
    }
    
    // Spawn stage manager task (for variable load)
    let stage_manager_handle = if is_variable_load {
        let current_stage_clone = Arc::clone(&current_stage);
        let ctx_clone = Arc::clone(&ctx);
        let quit_rx = ctx_clone.recv();
        let load_stages_clone = load_stages.clone();
        
        Some(tokio::spawn(async move {
            stage_manager_task(
                current_stage_clone,
                load_stages_clone,
                test_start,
                quit_rx,
            ).await
        }))
    } else {
        None
    };
    
    // Spawn metrics reporter task
    let stats_clone = Arc::clone(&stats);
    let current_stage_clone = Arc::clone(&current_stage);
    let ctx_clone = Arc::clone(&ctx);
    let quit_rx = ctx_clone.recv();
    let metrics_handle = tokio::spawn(async move {
        metrics_reporter_task(stats_clone, current_stage_clone, test_start, quit_rx).await
    });
    
    // Wait for test duration or interrupt signal
    let mut quit_rx = ctx.recv();
    tokio::select! {
        _ = tokio::time::sleep(test_duration) => {
            tracing::info!("Test duration of {} seconds reached", total_duration);
        }
        _ = quit_rx.recv() => {
            tracing::warn!("Received interrupt signal, shutting down...");
        }
    }
    
    // Wait for all sender tasks to complete
    tracing::info!("Waiting for sender tasks to complete...");
    let sender_results = join_all(sender_handles).await;
    
    // Stop stage manager and metrics reporter
    ctx.stop();
    if let Some(handle) = stage_manager_handle {
        let _ = handle.await;
    }
    let _ = metrics_handle.await;
    
    // Aggregate final statistics
    let stats_guard = stats.lock().unwrap();
    let mut total_sent = 0u64;
    let mut total_failed = 0u64;
    
    tracing::info!("=== FINAL STATISTICS ===");
    for (node_idx, node_stats) in stats_guard.iter() {
        let elapsed = test_start.elapsed().as_secs_f64();
        let throughput = if elapsed > 0.0 {
            node_stats.sent as f64 / elapsed
        } else {
            0.0
        };
        
        tracing::info!("Node {}: {} tx sent, {} failed, {:.0} tx/s", 
            node_idx, node_stats.sent, node_stats.failed, throughput);
        
        total_sent += node_stats.sent;
        total_failed += node_stats.failed;
    }
    
    let total_elapsed = test_start.elapsed().as_secs_f64();
    let total_throughput = if total_elapsed > 0.0 {
        total_sent as f64 / total_elapsed
    } else {
        0.0
    };
    
    tracing::info!("TOTAL: {} tx sent, {} failed, {:.0} tx/s aggregate", 
        total_sent, total_failed, total_throughput);
    tracing::info!("========================");
    
    // Check for errors in sender tasks
    for result in sender_results {
        if let Err(e) = result {
            tracing::error!("Sender task error: {}", e);
        }
    }
    
    // Return results in compatible format
    Ok(vec![RoundResults {
        sent: total_sent as usize,
        failed: total_failed as usize,
    }])
}

/// Continuous sender task for a single node with dynamic rate adjustment
/// and automatic reconnection on connection failure.
async fn continuous_sender_task(
    node_idx: usize,
    node_url: String,
    maybe_transport: Option<Framed<TcpStream, LengthDelimitedCodec>>,
    initially_connected: bool,
    tx_size: usize,
    stats: Arc<Mutex<HashMap<usize, NodeStats>>>,
    current_stage: Arc<Mutex<LoadStageInfo>>,
    test_duration: Duration,
    test_start: Instant,
    mut quit_rx: tokio::sync::broadcast::Receiver<()>,
) {
    use bytes::{BufMut, BytesMut};
    use futures::sink::SinkExt;
    use rand::Rng;

    let mut transport = maybe_transport;

    // Get initial rate
    let initial_rate = current_stage.lock().unwrap().target_tx_per_second;
    let mut current_rate = initial_rate;
    let mut interval_us = 1_000_000 / current_rate;
    let mut send_interval = Duration::from_micros(interval_us);

    tracing::debug!("Node {} sender starting: {} tx/s (smooth sending, 1 tx every {}µs)",
        node_idx, current_rate, interval_us);

    let mut interval_timer = interval(send_interval);
    interval_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut r: u64 = rand::rng().random();
    let mut tx_count: u64 = 0;
    let mut last_stage_check = Instant::now();
    let mut connected = initially_connected;

    loop {
        // Check if we should stop
        if test_start.elapsed() >= test_duration {
            tracing::debug!("Node {} sender: test duration reached, stopping", node_idx);
            break;
        }

        // If disconnected, attempt reconnection with exponential backoff
        if !connected {
            match reconnect_with_backoff(
                node_idx,
                &node_url,
                test_duration,
                test_start,
                &mut quit_rx,
            ).await {
                Some(new_transport) => {
                    transport = Some(new_transport);
                    connected = true;
                    // Reset interval timer after reconnection to avoid burst
                    interval_timer = interval(send_interval);
                    interval_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    tracing::info!("Node {} sender: resuming sending after reconnection", node_idx);
                }
                None => {
                    // Either test ended or quit signal received during reconnection
                    break;
                }
            }
        }

        // Periodically check if the stage has changed (every 100ms)
        if last_stage_check.elapsed() >= Duration::from_millis(100) {
            let stage_info = current_stage.lock().unwrap();
            let new_rate = stage_info.target_tx_per_second;
            drop(stage_info);

            if new_rate != current_rate {
                current_rate = new_rate;
                interval_us = 1_000_000 / current_rate;
                send_interval = Duration::from_micros(interval_us);

                // Create new interval timer with updated rate
                interval_timer = interval(send_interval);
                interval_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                tracing::debug!("Node {} sender: rate changed to {} tx/s (1 tx every {}µs)",
                    node_idx, current_rate, interval_us);
            }

            last_stage_check = Instant::now();
        }

        tokio::select! {
            _ = interval_timer.tick() => {
                let mut tx = BytesMut::with_capacity(tx_size);

                // Autobahn transaction format
                r += 1;
                tx.put_u8(1u8); // Standard transaction
                tx.put_u64(r);
                tx.resize(tx_size, 0u8);

                let bytes = tx.split().freeze();

                // Send transaction
                match transport.as_mut().unwrap().send(bytes).await {
                    Ok(_) => {
                        let mut stats_guard = stats.lock().unwrap();
                        if let Some(node_stats) = stats_guard.get_mut(&node_idx) {
                            node_stats.sent += 1;
                            node_stats.bytes_sent += tx_size as u64;
                        }
                        tx_count += 1;
                    }
                    Err(e) => {
                        tracing::warn!("Node {} send error: {} — will attempt reconnection", node_idx, e);
                        let mut stats_guard = stats.lock().unwrap();
                        if let Some(node_stats) = stats_guard.get_mut(&node_idx) {
                            node_stats.failed += 1;
                        }
                        transport = None;
                        connected = false;
                    }
                }
            }
            _ = quit_rx.recv() => {
                tracing::debug!("Node {} sender: received quit signal", node_idx);
                break;
            }
        }
    }

    tracing::debug!("Node {} sender task completed ({} transactions sent)", node_idx, tx_count);
}

/// Attempt to reconnect to a node with exponential backoff.
/// Returns Some(transport) if reconnected, None if the test ended or quit was signalled.
async fn reconnect_with_backoff(
    node_idx: usize,
    node_url: &str,
    test_duration: Duration,
    test_start: Instant,
    quit_rx: &mut tokio::sync::broadcast::Receiver<()>,
) -> Option<Framed<TcpStream, LengthDelimitedCodec>> {
    let mut backoff = Duration::from_millis(500);
    let max_backoff = Duration::from_secs(5);
    let mut attempt = 0u32;

    loop {
        if test_start.elapsed() >= test_duration {
            tracing::debug!("Node {} reconnect: test duration reached, giving up", node_idx);
            return None;
        }

        attempt += 1;
        tracing::info!("Node {} reconnect: attempt {} (backoff {:?})", node_idx, attempt, backoff);

        // Wait for backoff period or quit signal
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = quit_rx.recv() => {
                tracing::debug!("Node {} reconnect: quit signal received", node_idx);
                return None;
            }
        }

        // Check time again after sleeping
        if test_start.elapsed() >= test_duration {
            return None;
        }

        match TcpStream::connect(node_url).await {
            Ok(stream) => {
                if let Err(e) = stream.set_nodelay(true) {
                    tracing::warn!("Node {} reconnect: failed to set TCP_NODELAY: {}", node_idx, e);
                }
                tracing::info!("Node {} reconnect: success after {} attempts", node_idx, attempt);
                return Some(Framed::new(stream, LengthDelimitedCodec::new()));
            }
            Err(e) => {
                tracing::debug!("Node {} reconnect: attempt {} failed: {}", node_idx, attempt, e);
                backoff = (backoff * 2).min(max_backoff);
            }
        }
    }
}

/// Stage manager task - manages transitions between load stages
async fn stage_manager_task(
    current_stage: Arc<Mutex<LoadStageInfo>>,
    load_stages: Vec<LoadStage>,
    _test_start: Instant,
    mut quit_rx: tokio::sync::broadcast::Receiver<()>,
) {
    let mut check_interval = interval(Duration::from_millis(100));
    check_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    
    tracing::debug!("Stage manager started with {} stages", load_stages.len());
    
    loop {
        tokio::select! {
            _ = check_interval.tick() => {
                let mut stage_info = current_stage.lock().unwrap();
                let elapsed_in_stage = stage_info.stage_start.elapsed();
                
                // Check if current stage has completed
                if elapsed_in_stage >= stage_info.stage_duration {
                    let next_stage_idx = stage_info.stage_index + 1;
                    
                    // Check if there's a next stage
                    if next_stage_idx < load_stages.len() {
                        let next_stage = &load_stages[next_stage_idx];
                        
                        tracing::info!("=== STAGE TRANSITION: Stage {} → Stage {} ===", 
                            stage_info.stage_index + 1, 
                            next_stage_idx + 1);
                        tracing::info!("  New target: {} tx/s per node", next_stage.target_tx_per_second);
                        tracing::info!("  Duration: {} seconds", next_stage.duration_seconds);
                        
                        // Update to next stage
                        *stage_info = LoadStageInfo {
                            stage_index: next_stage_idx,
                            target_tx_per_second: next_stage.target_tx_per_second,
                            stage_start: Instant::now(),
                            stage_duration: Duration::from_secs(next_stage.duration_seconds),
                        };
                    } else {
                        // All stages completed
                        tracing::debug!("All stages completed");
                        break;
                    }
                }
            }
            _ = quit_rx.recv() => {
                tracing::debug!("Stage manager: received quit signal");
                break;
            }
        }
    }
    
    tracing::debug!("Stage manager task completed");
}

/// Metrics reporter task - reports stats every 1 second
async fn metrics_reporter_task(
    stats: Arc<Mutex<HashMap<usize, NodeStats>>>,
    current_stage: Arc<Mutex<LoadStageInfo>>,
    test_start: Instant,
    mut quit_rx: tokio::sync::broadcast::Receiver<()>,
) {
    let mut report_interval = interval(Duration::from_secs(1));
    report_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    
    let mut last_reported_stats: HashMap<usize, (u64, Instant)> = HashMap::new();
    
    loop {
        tokio::select! {
            _ = report_interval.tick() => {
                let now = Instant::now();
                let stats_guard = stats.lock().unwrap();
                
                // Get current stage info
                let stage_info = current_stage.lock().unwrap();
                let current_target = stage_info.target_tx_per_second;
                let stage_idx = stage_info.stage_index;
                let stage_elapsed = stage_info.stage_start.elapsed();
                let stage_remaining = stage_info.stage_duration.saturating_sub(stage_elapsed);
                drop(stage_info);
                
                let mut total_tx_per_sec = 0.0;
                let mut node_metrics = Vec::new();
                
                for (node_idx, node_stats) in stats_guard.iter() {
                    // Calculate instantaneous throughput since last report
                    let default_last = (0u64, test_start);
                    let (last_sent, last_time) = last_reported_stats
                        .get(node_idx)
                        .unwrap_or(&default_last);
                    
                    let tx_since_last = node_stats.sent.saturating_sub(*last_sent);
                    let time_since_last = now.duration_since(*last_time).as_secs_f64();
                    
                    let throughput = if time_since_last > 0.0 {
                        tx_since_last as f64 / time_since_last
                    } else {
                        0.0
                    };
                    
                    total_tx_per_sec += throughput;
                    node_metrics.push((*node_idx, node_stats.sent, node_stats.failed, throughput));
                }
                
                drop(stats_guard);
                
                // Update last reported stats
                for (node_idx, sent, _, _) in &node_metrics {
                    last_reported_stats.insert(*node_idx, (*sent, now));
                }
                
                // Log metrics with stage info
                tracing::info!("=== Metrics at T+{:.1}s | Stage {} | Target: {} tx/s | Stage time remaining: {:.0}s ===", 
                    test_start.elapsed().as_secs_f64(),
                    stage_idx + 1,
                    current_target,
                    stage_remaining.as_secs_f64());
                
                let num_nodes = node_metrics.len();
                    
                for (node_idx, sent, failed, throughput) in &node_metrics {
                    let target_match = (throughput / current_target as f64 * 100.0) as i32;
                    let marker = if *throughput < current_target as f64 * 0.8 { 
                        " ⚠️ SLOW" 
                    } else if *throughput > current_target as f64 * 1.2 {
                        " ⚡ FAST"
                    } else { 
                        "" 
                    };
                    tracing::info!("  Node {}: {:.0} tx/s ({}% of target, {} sent, {} failed){}",
                        node_idx, throughput, target_match, sent, failed, marker);
                }
                tracing::info!("  TOTAL: {:.0} tx/s aggregate ({:.0}% of target {})", 
                    total_tx_per_sec,
                    total_tx_per_sec / (current_target * num_nodes as u64) as f64 * 100.0,
                    current_target * num_nodes as u64);
            }
            _ = quit_rx.recv() => {
                tracing::debug!("Metrics reporter: received quit signal");
                break;
            }
        }
    }
    
    tracing::debug!("Metrics reporter task completed");
}

/// LEGACY: Original batch-and-wait mode (for backward compatibility)
async fn run_legacy_batch_mode(
    ctx: Arc<ctx::Context>,
    cfg: config::Config,
    rpc_urls: Vec<String>,
) -> Result<Vec<RoundResults>, TestrpcError> {
    let mut i: u32 = 0;
    let mut quit = ctx.recv();
    let results = Arc::new(RwLock::new(Vec::new()));
    
    tracing::info!("Runner starting in LEGACY BATCH MODE with {} iterations configured, {} rounds per iteration, {} nodes",
        cfg.iterations.map(|i| i.to_string()).unwrap_or_else(|| "unlimited".to_string()),
        cfg.rounds.len(),
        rpc_urls.len()
    );
    
    loop {
        tracing::debug!("Starting main loop iteration {}", i + 1);
        let rounds = cfg.rounds.clone();
        for (r, round) in rounds.into_iter().enumerate() {
            let round_templates = cfg.round_templates.clone();
            let rpc_urls = rpc_urls.clone();
            let results = Arc::clone(&results);
            i += 1;
            let iteration = i;
            let round_num = r;
            let adapter = cfg.adapter.clone();
            tokio::select! {
                result = process_round(adapter, round, iteration, rpc_urls, round_templates) => {
                    match result {
                        Ok(result) => {
                            // Detailed logging is now in process_round
                            let mut results = results.write().unwrap();
                            results.push(result);
                        }
                        Err(e) => {
                            tracing::warn!("Iteration {} round {} failed: {}", iteration, round_num, e);
                        }
                    }
                }
                _ = quit.recv() => {
                    tracing::warn!("Context stopped signal received during iteration {} round {} - terminating early", iteration, round_num);
                    break;
                }
            }
            tokio::select! {
                _ = quit.recv() => {
                    tracing::warn!("Context stopped signal received during interval sleep (iteration {} round {}) - terminating early", iteration, round_num);
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(cfg.interval)) => {
                    tracing::debug!("Completed interval sleep of {} seconds", cfg.interval);
                }
            }
            if let Some(iterations) = cfg.iterations {
                if i >= iterations as u32 {
                    tracing::info!("Reached configured max iterations: {} (target was {})", i, iterations);
                    break;
                }
            }
        }
        if let Some(iterations) = cfg.iterations {
            if i >= iterations as u32 {
                tracing::info!("Exiting main loop: reached configured max iterations {} (target was {})", i, iterations);
                break;
            }
        }
    }
    
    tracing::info!("Runner completed after {} iterations", i);
    let results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
    tracing::info!("Collected {} round results", results.len());
    Ok(results)
}

/// Process a single round, sending transactions to the RPC servers concurrently
async fn process_round(
    cfg: AdapterConfig,
    round: config::Round,
    iteration: u32,
    rpc_urls: Vec<String>,
    round_templates: HashMap<String, config::RoundTemplate>,
) -> Result<RoundResults, TestrpcError> {
    let mut req_id = iteration as u64;
    let mut results = RoundResults { sent: 0, failed: 0 };
    let mut handles = Vec::new();

    let adapter = adapters::new_adapter(cfg)?;
    
    let round_start = std::time::Instant::now();

    let mut node_timings = Vec::new();
    
    for rpc in &round.rpcs {
        if rpc_urls.len() <= *rpc {
            return Err(TestrpcError::LoadEndpointsError(format!(
                "RPC index out of bounds: {rpc}"
            )));
        }
        let rpc_url = rpc_urls[*rpc].clone();
        let req_id_clone = req_id;

        let template = round.get_template(round_templates.clone()).ok_or(
            TestrpcError::LoadRoundTemplateError("No template found".to_string()),
        )?;

        let adapter = adapter.clone();
        let node_url = rpc_url.clone();
        let node_start = std::time::Instant::now();
        
        let handle = tokio::spawn(async move {
            let result = adapter
                .send_txs(
                    &rpc_url,
                    req_id_clone,
                    iteration,
                    template.txs,
                    template.tx_size,
                )
                .await;
            (node_url, node_start.elapsed(), result)
        });

        handles.push(handle);
        req_id += 1;
    }

    let results_vec = join_all(handles).await;
    
    // Track node timings for slowest node identification
    let mut slowest_node = String::new();
    let mut slowest_time = Duration::from_secs(0);

    for result in results_vec {
        match result {
            Ok((node_url, duration, Ok(round_results))) => {
                results.sent += round_results.sent;
                results.failed += round_results.failed;
                
                // Track timing for this node
                node_timings.push((node_url.clone(), duration));
                
                // Update slowest node
                if duration > slowest_time {
                    slowest_time = duration;
                    slowest_node = node_url;
                }
            }
            Ok((_, _, Err(e))) => return Err(e),
            Err(e) => return Err(TestrpcError::ExecutionError(e.to_string())),
        }
    }
    
    let round_duration = round_start.elapsed();
    
    // Log performance summary with slowest node highlighted
    tracing::info!(
        "Iteration {} completed in {:?} (slowest node: {} took {:?})", 
        iteration, 
        round_duration,
        slowest_node,
        slowest_time
    );
    
    // Detailed per-node timing at debug level
    for (node, duration) in node_timings {
        tracing::debug!("  Node {} took {:?}", node, duration);
    }
    
    Ok(results)
}

#[cfg(test)]
mod tests {
    use crate::config::{Round, RoundTemplate};

    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_process_round() {
        // set DRY_RUN to avoid sending requests
        std::env::set_var("DRY_RUN", "true");
        let round = Round {
            rpcs: vec![0],
            repeat: Some(1),
            template: Some(RoundTemplate {
                txs: 1,
                tx_size: 1,
                latency: None,
            }),
            use_template: None,
        };
        let rpc_urls = vec!["http://localhost:5000".to_string()];
        let round_templates = HashMap::new();
        let results = process_round(
            config::AdapterConfig::Hotshot,
            round,
            0,
            rpc_urls,
            round_templates,
        )
        .await
        .unwrap();
        assert_eq!(results.sent, 1);
        assert_eq!(results.failed, 0);
    }
}
