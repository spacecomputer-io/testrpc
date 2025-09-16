use futures::future::join_all;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::task;
use tokio::time::Duration;

use crate::adapters::Adapter;
use crate::common::{RoundResults, TestrpcError};
use crate::config::{self, AdapterConfig};
use crate::{adapters, ctx};

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
    let mut i: u32 = 0;
    let mut quit = ctx.recv();
    let results = Arc::new(RwLock::new(Vec::new()));
    loop {
        let rounds = cfg.rounds.clone();
        for (r, round) in rounds.into_iter().enumerate() {
            let round_templates = cfg.round_templates.clone();
            let rpc_urls = rpc_urls.clone();
            let results = Arc::clone(&results);
            i += 1;
            let iteration = i;
            let round_num = r;
            let adapter = cfg.adapter.clone();
            let timeout = cfg.timeout.map(|t| Duration::from_secs(t as u64));
            tokio::select! {
                _ = task::spawn(async move {
                    match process_round(adapter, round, iteration, rpc_urls, round_templates, timeout).await {
                        Ok(result) => {
                            tracing::debug!("Iteration {} round {} completed", iteration, round_num);
                            let mut results = results.write().unwrap();
                            results.push(result);
                        }
                        Err(e) => {
                            tracing::warn!("Iteration {} round {} failed: {}", iteration, round_num, e);
                        }
                    }
                }) => {}
                _ = quit.recv() => {
                    tracing::debug!("Iteration {} round {} timed out as ctx was stopped", iteration, round_num);
                    break;
                }
            }
            tokio::select! {
                _ = quit.recv() => {
                    tracing::debug!("ctx stopped during iteration {} round {}", iteration, round_num);
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(cfg.interval)) => {}
            }
            if let Some(iterations) = cfg.iterations {
                if i >= iterations as u32 {
                    tracing::debug!("Reached max iterations: {}", i);
                    break;
                }
            }
        }
        if let Some(iterations) = cfg.iterations {
            if i >= iterations as u32 {
                tracing::debug!("Reached max iterations: {}", i);
                break;
            }
        }
    }
    let results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
    Ok(results)
}

/// Process a single round, sending transactions to the RPC servers concurrently
async fn process_round(
    cfg: AdapterConfig,
    round: config::Round,
    iteration: u32,
    rpc_urls: Vec<String>,
    round_templates: HashMap<String, config::RoundTemplate>,
    timeout: Option<std::time::Duration>,
) -> Result<RoundResults, TestrpcError> {
    let mut req_id = iteration as u64;
    let mut results = RoundResults { sent: 0, failed: 0 };
    let mut handles = Vec::new();

    let adapter = adapters::new_adapter(cfg)?;

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
        let handle = tokio::spawn(async move {
            adapter
                .send_txs(
                    &rpc_url,
                    req_id_clone,
                    iteration,
                    template.txs,
                    template.tx_size,
                    timeout,
                )
                .await
        });

        handles.push(handle);
        req_id += 1;
    }

    let results_vec = join_all(handles).await;

    for result in results_vec {
        match result {
            Ok(Ok(round_results)) => {
                results.sent += round_results.sent;
                results.failed += round_results.failed;
            }
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(TestrpcError::ExecutionError(e.to_string())),
        }
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
            Some(std::time::Duration::from_secs(5)),
        )
        .await
        .unwrap();
        assert_eq!(results.sent, 1);
        assert_eq!(results.failed, 0);
    }
}
