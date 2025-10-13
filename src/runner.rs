use futures::future::join_all;
use std::collections::HashMap;
use std::sync::{Arc, RwLock, Mutex};
use tokio::time::{Duration, Instant, interval};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use crate::common::{RoundResults, TestrpcError};
use crate::config::{self, AdapterConfig};
use crate::{adapters, ctx};

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
    let use_continuous_mode = cfg.duration_seconds.is_some() && cfg.target_tx_per_second.is_some();
    
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
    let duration_seconds = cfg.duration_seconds.unwrap();
    let target_tx_per_second = cfg.target_tx_per_second.unwrap();
    
    tracing::info!("Runner starting in CONTINUOUS MODE:");
    tracing::info!("  Duration: {} seconds", duration_seconds);
    tracing::info!("  Target: {} tx/s per node", target_tx_per_second);
    tracing::info!("  Nodes: {}", rpc_urls.len());
    tracing::info!("  Total target throughput: {} tx/s", target_tx_per_second * rpc_urls.len() as u64);
    
    // Get transaction template
    let template = cfg.rounds.first()
        .and_then(|r| r.get_template(cfg.round_templates.clone()))
        .ok_or_else(|| TestrpcError::LoadRoundTemplateError("No template found".to_string()))?;
    
    let tx_size = template.tx_size;
    
    // Check for dry-run mode
    let is_dry_run = std::env::var("DRY_RUN").is_ok();
    if is_dry_run {
        tracing::info!("DRY_RUN mode: Would establish persistent connections to {} nodes", rpc_urls.len());
        tracing::info!("DRY_RUN mode: Would send {} tx/s per node for {} seconds", target_tx_per_second, duration_seconds);
        tracing::info!("DRY_RUN mode: Total expected transactions: {} per node, {} aggregate",
            target_tx_per_second * duration_seconds,
            target_tx_per_second * duration_seconds * rpc_urls.len() as u64);
        
        return Ok(vec![RoundResults {
            sent: (target_tx_per_second * duration_seconds * rpc_urls.len() as u64) as usize,
            failed: 0,
        }]);
    }
    
    // Establish persistent connections to all nodes
    tracing::info!("Establishing persistent connections to {} nodes...", rpc_urls.len());
    let mut connections = Vec::new();
    
    for (idx, url) in rpc_urls.iter().enumerate() {
        match TcpStream::connect(url).await {
            Ok(stream) => {
                let framed = Framed::new(stream, LengthDelimitedCodec::new());
                connections.push((idx, url.clone(), framed));
                tracing::debug!("Connected to node {} ({})", idx, url);
            }
            Err(e) => {
                tracing::error!("Failed to connect to node {} ({}): {}", idx, url, e);
                return Err(TestrpcError::RpcError(format!("Failed to connect to {}: {}", url, e)));
            }
        }
    }
    
    tracing::info!("Successfully established {} persistent connections", connections.len());
    
    // Shared statistics across all sender tasks
    let stats = Arc::new(Mutex::new(HashMap::<usize, NodeStats>::new()));
    for (idx, _, _) in &connections {
        stats.lock().unwrap().insert(*idx, NodeStats::default());
    }
    
    // Spawn sender tasks for each node
    let mut sender_handles = Vec::new();
    let test_start = Instant::now();
    let test_duration = Duration::from_secs(duration_seconds);
    
    for (node_idx, node_url, transport) in connections {
        let stats_clone = Arc::clone(&stats);
        let ctx_clone = Arc::clone(&ctx);
        let quit_rx = ctx_clone.recv();
        
        let handle = tokio::spawn(async move {
            continuous_sender_task(
                node_idx,
                node_url,
                transport,
                target_tx_per_second,
                tx_size,
                stats_clone,
                test_duration,
                test_start,
                quit_rx,
            ).await
        });
        
        sender_handles.push(handle);
    }
    
    // Spawn metrics reporter task
    let stats_clone = Arc::clone(&stats);
    let ctx_clone = Arc::clone(&ctx);
    let quit_rx = ctx_clone.recv();
    let metrics_handle = tokio::spawn(async move {
        metrics_reporter_task(stats_clone, test_start, quit_rx).await
    });
    
    // Wait for test duration or interrupt signal
    let mut quit_rx = ctx.recv();
    tokio::select! {
        _ = tokio::time::sleep(test_duration) => {
            tracing::info!("Test duration of {} seconds reached", duration_seconds);
        }
        _ = quit_rx.recv() => {
            tracing::warn!("Received interrupt signal, shutting down...");
        }
    }
    
    // Wait for all sender tasks to complete
    tracing::info!("Waiting for sender tasks to complete...");
    let sender_results = join_all(sender_handles).await;
    
    // Stop metrics reporter
    ctx.stop();
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

/// Continuous sender task for a single node
async fn continuous_sender_task(
    node_idx: usize,
    _node_url: String,
    mut transport: Framed<TcpStream, LengthDelimitedCodec>,
    target_tx_per_second: u64,
    tx_size: usize,
    stats: Arc<Mutex<HashMap<usize, NodeStats>>>,
    test_duration: Duration,
    test_start: Instant,
    mut quit_rx: tokio::sync::broadcast::Receiver<()>,
) {
    use bytes::{BufMut, BytesMut};
    use futures::sink::SinkExt;
    use rand::Rng;
    
    const PRECISION: u64 = 20; // 20 bursts per second (50ms intervals)
    let burst_interval = Duration::from_millis(1000 / PRECISION);
    let burst_size = (target_tx_per_second / PRECISION) as usize;
    
    tracing::debug!("Node {} sender starting: {} tx/s ({} tx per {}ms burst)", 
        node_idx, target_tx_per_second, burst_size, burst_interval.as_millis());
    
    let mut interval_timer = interval(burst_interval);
    interval_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    
    let mut r: u64 = rand::rng().random();
    
    loop {
        // Check if we should stop
        if test_start.elapsed() >= test_duration {
            tracing::debug!("Node {} sender: test duration reached, stopping", node_idx);
            break;
        }
        
        tokio::select! {
            _ = interval_timer.tick() => {
                let burst_start = Instant::now();
                
                // Send burst of transactions
                for _ in 0..burst_size {
                    let mut tx = BytesMut::with_capacity(tx_size);
                    
                    // Autobahn transaction format
                    r += 1;
                    tx.put_u8(1u8); // Standard transaction
                    tx.put_u64(r);
                    tx.resize(tx_size, 0u8);
                    
                    let bytes = tx.split().freeze();
                    
                    // Send transaction
                    match transport.send(bytes).await {
                        Ok(_) => {
                            let mut stats_guard = stats.lock().unwrap();
                            if let Some(node_stats) = stats_guard.get_mut(&node_idx) {
                                node_stats.sent += 1;
                                node_stats.bytes_sent += tx_size as u64;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Node {} send error: {}", node_idx, e);
                            let mut stats_guard = stats.lock().unwrap();
                            if let Some(node_stats) = stats_guard.get_mut(&node_idx) {
                                node_stats.failed += 1;
                            }
                            // Continue sending to other nodes even if this one fails
                            break;
                        }
                    }
                }
                
                // Warn if burst took longer than expected
                let burst_duration = burst_start.elapsed();
                if burst_duration > burst_interval {
                    tracing::warn!("Node {} burst took {:?}, exceeds target of {:?}", 
                        node_idx, burst_duration, burst_interval);
                }
            }
            _ = quit_rx.recv() => {
                tracing::debug!("Node {} sender: received quit signal", node_idx);
                break;
            }
        }
    }
    
    tracing::debug!("Node {} sender task completed", node_idx);
}

/// Metrics reporter task - reports stats every 1 second
async fn metrics_reporter_task(
    stats: Arc<Mutex<HashMap<usize, NodeStats>>>,
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
                
                // Log metrics
                tracing::info!("=== Metrics at T+{:.1}s ===", test_start.elapsed().as_secs_f64());
                for (node_idx, sent, failed, throughput) in node_metrics {
                    let marker = if throughput < 20000.0 { " ⚠️ SLOW" } else { "" };
                    tracing::info!("  Node {}: {:.0} tx/s ({} sent, {} failed){}",
                        node_idx, throughput, sent, failed, marker);
                }
                tracing::info!("  TOTAL: {:.0} tx/s aggregate", total_tx_per_sec);
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
