use futures::future::join_all;
use libp2p::Multiaddr;
use rand::Rng as _;
use serde_yaml::Value;
use std::collections::HashMap;

use crate::common::{RoundResults, TestrpcError};
use crate::config::{Round, RoundTemplate};
use crate::jrpc;

const RPC_METHOD: &str = "send_txs";

/// Arguments for the Hotshot adapter
pub struct HotshotArgs {
    /// Coordinator URL to use for fetching the RPC endpoints
    pub coordinator_url: String,
}

impl TryFrom<HashMap<String, Value>> for HotshotArgs {
    type Error = TestrpcError;

    fn try_from(args: HashMap<String, Value>) -> Result<Self, Self::Error> {
        let coordinator_url = match args.get("coordinator_url") {
            Some(Value::String(coordinator_url)) => coordinator_url.clone(),
            _ => return Err(TestrpcError::MissingArgs("coordinator_url".to_string())),
        };

        Ok(HotshotArgs { coordinator_url })
    }
}

/// Load the RPC endpoints from the coordinator
pub async fn load_endpoints(args: HashMap<String, Value>) -> Result<Vec<String>, TestrpcError> {
    let HotshotArgs { coordinator_url } = HotshotArgs::try_from(args)?;

    tracing::info!("Using coordinator at: {}", coordinator_url.clone());
    // Fetch the known libp2p nodes from the coordinator
    let p2p_info_url = format!("http://{}/libp2p-info", coordinator_url);
    let resp = reqwest::get(p2p_info_url.as_str())
        .await
        .map_err(|e| TestrpcError::LoadEndpointsError(e.to_string()))?
        .text()
        .await
        .map_err(|e| TestrpcError::LoadEndpointsError(e.to_string()))?;
    let known_ips = parse_endpoints(resp.as_str());
    // Print the known libp2p nodes
    tracing::info!("Known libp2p nodes: {:?}", known_ips);

    let rpc_urls = known_ips
        .iter()
        .map(|ip| format!("http://{}:5000", ip.clone()))
        .collect::<Vec<_>>();

    Ok(rpc_urls)
}

fn parse_endpoints(endpoints: &str) -> Vec<String> {
    endpoints
        .split('\n')
        .map(|s| {
            s.parse::<Multiaddr>()
                .map_err(|e| TestrpcError::LoadEndpointsError(e.to_string()))
                .unwrap()
        })
        .map(|addr| {
            let components = addr.iter().collect::<Vec<_>>();
            if components.len() < 2 {
                return addr.to_string();
            }
            let ip_addr = components[0].to_string();
            if ip_addr.len() < 6 {
                return addr.to_string();
            }
            // "/ip4/*.*.*.*" -> "*.*.*.*"
            ip_addr[5..].to_string()
        })
        .collect::<Vec<_>>()
}

/// Process a single round, sending transactions to the RPC servers concurrently
pub async fn process_round(
    round: Round,
    iteration: u32,
    rpc_urls: Vec<String>,
    round_templates: HashMap<String, RoundTemplate>,
) -> Result<RoundResults, TestrpcError> {
    let mut req_id = iteration as u64;
    let mut results = RoundResults { sent: 0, failed: 0 };
    let mut handles = Vec::new();

    for rpc in &round.rpcs {
        if rpc_urls.len() <= *rpc {
            return Err(TestrpcError::LoadEndpointsError(format!(
                "RPC index out of bounds: {}",
                rpc
            )));
        }
        let rpc_url = rpc_urls[*rpc].clone();
        let req_id_clone = req_id;

        let template = round.get_template(round_templates.clone()).ok_or(
            TestrpcError::LoadRoundTemplateError("No template found".to_string()),
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
            Err(e) => return Err(TestrpcError::ExecutionError(e.to_string())),
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
) -> Result<RoundResults, TestrpcError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_parse_endpoints() {
        let resp = r#"/ip4/192.168.104.3/udp/3000/quic-v1/p2p/12D3KooWPnJybf5PYvQBYeVrFPRR4BfzPzHohdtBp5R4372CPcNp
/ip4/192.168.104.4/udp/3000/quic-v1/p2p/12D3KooWSe24subEEphVfaCzuQhZtmKRpAqbNm12BNFkCPe2fauF
/ip4/192.168.104.5/udp/3000/quic-v1/p2p/12D3KooWMhCH2B3bWm9TVzvtntPVMyctNgiNb2GKKWFjxBxqD1md"#;
        let known_ips = parse_endpoints(resp);
        assert_eq!(known_ips.len(), 3);
        assert_eq!(known_ips[0], "192.168.104.3");
        assert_eq!(known_ips[1], "192.168.104.4");
        assert_eq!(known_ips[2], "192.168.104.5");
    }

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
        let results = process_round(round, 0, rpc_urls, round_templates)
            .await
            .unwrap();
        assert_eq!(results.sent, 1);
        assert_eq!(results.failed, 0);
    }
}
