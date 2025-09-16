/// Hotshot implementation of the adapter
use crate::adapters::Adapter;
use libp2p::Multiaddr;
use rand::Rng as _;
use serde_yaml::Value;
use std::collections::HashMap;

use crate::common::{RoundResults, TestrpcError};
use crate::jrpc;

const RPC_METHOD: &str = "send_txs";

/// Arguments for the Hotshot adapter
pub struct HotshotArgs {
    /// Coordinator URL to use for fetching the RPC endpoints
    pub coordinator_url: String,
    /// RPC port to use for sending transactions
    pub rpc_port: u16,
}

impl TryFrom<HashMap<String, Value>> for HotshotArgs {
    type Error = TestrpcError;

    fn try_from(args: HashMap<String, Value>) -> Result<Self, Self::Error> {
        let coordinator_url = match args.get("coordinator_url") {
            Some(Value::String(coordinator_url)) => coordinator_url.clone(),
            _ => return Err(TestrpcError::MissingArgs("coordinator_url".to_string())),
        };
        let rpc_port = match args.get("rpc_port") {
            Some(Value::Number(port)) if port.is_u64() => port.as_u64().unwrap() as u16,
            _ => 5000,
        };

        Ok(HotshotArgs {
            coordinator_url,
            rpc_port,
        })
    }
}

pub struct HotshotAdapter;

impl HotshotAdapter {
    pub fn new() -> Self {
        HotshotAdapter {}
    }
}

impl Default for HotshotAdapter {
    fn default() -> Self {
        HotshotAdapter::new()
    }
}

impl Adapter for HotshotAdapter {
    async fn load_endpoints(
        &self,
        args: HashMap<String, Value>,
    ) -> Result<Vec<String>, TestrpcError> {
        let HotshotArgs {
            coordinator_url,
            rpc_port,
        } = HotshotArgs::try_from(args)?;
        tracing::info!("Using coordinator at: {}", coordinator_url.clone());
        // Fetch the known libp2p nodes from the coordinator
        let p2p_info_url = format!("http://{coordinator_url}/libp2p-info");
        let resp = reqwest::get(p2p_info_url.as_str())
            .await
            .map_err(|e| TestrpcError::LoadEndpointsError(e.to_string()))?
            .text()
            .await
            .map_err(|e| TestrpcError::LoadEndpointsError(e.to_string()))?;
        let known_ips = parse_endpoints(resp.as_str())?;
        // Print the known libp2p nodes
        tracing::info!("Known libp2p nodes: {:?}", known_ips);

        let rpc_urls = known_ips
            .iter()
            .map(|ip| format!("http://{ip}:{rpc_port}"))
            .collect::<Vec<_>>();

        if rpc_urls.is_empty() {
            return Err(TestrpcError::LoadEndpointsError(
                "No RPC endpoints found".to_string(),
            ));
        }
        Ok(rpc_urls)
    }

    async fn ping_endpoint(
        &self,
        rpc_url: &str,
        timeout: Option<std::time::Duration>,
    ) -> Result<bool, crate::common::TestrpcError> {
        let req_id = rand::rng().random::<u64>();
        let _ = jrpc::send(rpc_url, req_id, RPC_METHOD, serde_json::json!({}), timeout).await?;

        Ok(true)
    }

    async fn send_txs(
        &self,
        rpc_url: &str,
        req_id: u64,
        _iteration: u32,
        num_txs: usize,
        tx_size: usize,
        timeout: Option<std::time::Duration>,
    ) -> Result<RoundResults, TestrpcError> {
        let mut txs: Vec<String> = Vec::new();
        for _ in 0..num_txs {
            let mut transaction_bytes = vec![0u8; tx_size];
            rand::rng().fill(&mut transaction_bytes[..]);
            txs.push(hex::encode(transaction_bytes));
        }
        let _ = jrpc::send(
            rpc_url,
            req_id,
            RPC_METHOD,
            serde_json::json!({ "txs": txs }),
            timeout,
        )
        .await?;

        Ok(RoundResults {
            sent: num_txs,
            failed: 0,
        })
    }
}

fn parse_endpoints(endpoints: &str) -> Result<Vec<String>, TestrpcError> {
    let endpoints = endpoints
        .split('\n')
        .map(|s| {
            s.parse::<Multiaddr>()
                .map_err(|e| TestrpcError::LoadEndpointsError(e.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter();

    let addrs = endpoints
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
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();

    Ok(addrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_endpoints() {
        let resp = r#"/ip4/192.168.104.3/udp/3000/quic-v1/p2p/12D3KooWPnJybf5PYvQBYeVrFPRR4BfzPzHohdtBp5R4372CPcNp
/ip4/192.168.104.4/udp/3000/quic-v1/p2p/12D3KooWSe24subEEphVfaCzuQhZtmKRpAqbNm12BNFkCPe2fauF
/ip4/192.168.104.5/udp/3000/quic-v1/p2p/12D3KooWMhCH2B3bWm9TVzvtntPVMyctNgiNb2GKKWFjxBxqD1md"#;
        let known_ips = parse_endpoints(resp).unwrap();
        assert_eq!(known_ips.len(), 3);
        assert_eq!(known_ips[0], "192.168.104.3");
        assert_eq!(known_ips[1], "192.168.104.4");
        assert_eq!(known_ips[2], "192.168.104.5");
    }
}
