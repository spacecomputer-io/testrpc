use futures::future::join_all;
use libp2p::Multiaddr;
use rand::Rng as _;
use std::collections::HashMap;

use crate::common::{RoundResults, TestflowError};
use crate::config::{Round, RoundTemplate};
use crate::jrpc;

const RPC_METHOD: &str = "send_txs";

/// Arguments for the Hotshot adapter
pub struct HotshotArgs {
    /// Coordinator URL to use for fetching the RPC endpoints
    pub coordinator_url: String,
}

impl TryFrom<HashMap<String, String>> for HotshotArgs {
    type Error = TestflowError;

    fn try_from(args: HashMap<String, String>) -> Result<Self, Self::Error> {
        let coordinator_url = args
            .get("coordinator_url")
            .ok_or(TestflowError::MissingArgs("coordinator_url".to_string()))?
            .clone();

        Ok(HotshotArgs { coordinator_url })
    }
}

/// Load the RPC endpoints from the coordinator
pub async fn load_endpoints(args: HashMap<String, String>) -> Result<Vec<String>, TestflowError> {
    let HotshotArgs { coordinator_url } = HotshotArgs::try_from(args)?;
    tracing::info!("Using coordinator at: {}", coordinator_url.clone());
    // Fetch the known libp2p nodes from the coordinator
    let p2p_info_url = format!("http://{}/libp2p-info", coordinator_url);
    let known_ips = reqwest::get(p2p_info_url.as_str())
        .await
        .map_err(|e| TestflowError::LoadEndpointsError(e.to_string()))?
        .text()
        .await
        .map_err(|e| TestflowError::LoadEndpointsError(e.to_string()))?
        .split('\n')
        .map(|s| {
            s.parse::<Multiaddr>()
                .map_err(|e| TestflowError::LoadEndpointsError(e.to_string()))
                .unwrap()
        })
        .map(|addr| {
            let components = addr.iter().collect::<Vec<_>>();
            if components.len() < 2 {
                return addr.to_string();
            }
            components[0].to_string()
        })
        .collect::<Vec<_>>();
    // Print the known libp2p nodes
    tracing::info!("Known libp2p nodes: {:?}", known_ips);

    let rpc_urls = known_ips
        .iter()
        .map(|ip| format!("http://{}:5000", ip.clone()))
        .collect::<Vec<_>>();

    Ok(rpc_urls)
}

/// Process a single round, sending transactions to the RPC servers concurrently
pub async fn process_round(
    round: Round,
    iteration: u32,
    rpc_urls: Vec<String>,
    round_templates: HashMap<String, RoundTemplate>,
) -> Result<RoundResults, TestflowError> {
    let mut req_id = iteration as u64;
    let mut results = RoundResults { sent: 0, failed: 0 };
    let mut handles = Vec::new();

    for rpc in &round.rpcs {
        if rpc_urls.len() <= *rpc {
            return Err(TestflowError::LoadEndpointsError(format!(
                "RPC index out of bounds: {}",
                rpc
            )));
        }
        let rpc_url = rpc_urls[*rpc].clone();
        let req_id_clone = req_id;

        let template = round.get_template(round_templates.clone()).ok_or(
            TestflowError::LoadRoundTemplateError("No template found".to_string()),
        )?;

        let handle = tokio::spawn(async move {
            send_txs(&rpc_url, req_id_clone, template.txs, template.tx_size).await
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
            Err(e) => return Err(TestflowError::ExecutionError(e.to_string())),
        }
    }
    Ok(results)
}

/// Sends transactions to the RPC server
async fn send_txs(
    rpc_url: &str,
    req_id: u64,
    n: usize,
    tx_size: usize,
) -> Result<RoundResults, TestflowError> {
    let mut txs: Vec<String> = Vec::new();
    for _ in 0..n {
        let mut transaction_bytes = vec![0u8; tx_size];
        rand::rng().fill(&mut transaction_bytes[..]);
        txs.push(hex::encode(transaction_bytes));
    }
    let _ = jrpc::send(
        rpc_url,
        req_id,
        RPC_METHOD,
        serde_json::json!({ "txs": txs }),
    )
    .await?;

    Ok(RoundResults { sent: n, failed: 0 })
}
