use crate::adapters::Adapter;

use rand::Rng as _;
use serde_yaml::Value;
use std::collections::HashMap;

use crate::common::{RoundResults, TestrpcError};

pub struct EvmArgs {
    /// List of RPC URLs to use for sending transactions
    pub rpc_urls: Vec<String>,
    /// Path to the fuzzer project to be called from this adapter
    pub fuzzer: String,
}

impl TryFrom<HashMap<String, Value>> for EvmArgs {
    type Error = TestrpcError;

    fn try_from(args: HashMap<String, Value>) -> Result<Self, Self::Error> {
        let rpc_urls = match args.get("rpc_urls") {
            Some(Value::Sequence(urls)) => urls
                .iter()
                .filter_map(|url| {
                    if let Value::String(s) = url {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            _ => return Err(TestrpcError::MissingArgs("rpc_urls".to_string())),
        };
        let fuzzer = match args.get("fuzzer") {
            Some(Value::String(fuzzer)) => fuzzer.clone(),
            _ => return Err(TestrpcError::MissingArgs("fuzzer".to_string())),
        };

        Ok(EvmArgs { rpc_urls, fuzzer })
    }
}

pub struct EvmAdapter {
    fuzzer: String,
}

impl EvmAdapter {
    pub fn new(args: HashMap<String, Value>) -> Self {
        let EvmArgs { fuzzer, .. } = EvmArgs::try_from(args).expect("Invalid arguments");
        EvmAdapter { fuzzer }
    }
}

impl Default for EvmAdapter {
    fn default() -> Self {
        let mut h = HashMap::new();
        h.insert(
            "fuzzer".to_string(),
            // Value::String("/home/user/space-computer-platform/fuzzer".to_string()),
            Value::String(
                "/Users/amirylm/dev/spacecomp/space-computer-platform/scripts/evm/fuzzer"
                    .to_string(),
            ),
        );
        h.insert(
            "rpc_urls".to_string(),
            Value::Sequence(
                // vec!["http://localhost:8545".to_string()]
                vec!["http://192.168.1.219:8545".to_string()]
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        EvmAdapter::new(h)
    }
}

impl Adapter for EvmAdapter {
    async fn load_endpoints(
        &self,
        args: HashMap<String, Value>,
    ) -> Result<Vec<String>, TestrpcError> {
        let EvmArgs {
            rpc_urls,
            fuzzer: _,
        } = EvmArgs::try_from(args)?;
        Ok(rpc_urls)
    }

    async fn ping_endpoint(
        &self,
        rpc_url: &str,
        timeout: Option<std::time::Duration>,
    ) -> Result<bool, TestrpcError> {
        let timeout = timeout.unwrap_or(std::time::Duration::from_secs(10));
        let child = tokio::process::Command::new("sh")
            .current_dir(&self.fuzzer)
            .env("RPC_URL", rpc_url)
            .arg("-c")
            .arg("npm run block")
            .spawn()
            .map_err(|e| TestrpcError::RpcError(e.to_string()))?;
        tokio::select! {
            output = child.wait_with_output() => {
                let output = output
                    .map_err(|e| TestrpcError::RpcError(e.to_string()))?;
                Ok(output.status.success())
            },
            _ = tokio::time::sleep(timeout) => Ok(false)
        }
    }

    async fn send_txs(
        &self,
        rpc_url: &str,
        _req_id: u64,
        _iteration: u32,
        num_txs: usize,
        tx_size: usize, // NOTE: used here to determine how many accounts to use
        timeout: Option<std::time::Duration>,
    ) -> Result<RoundResults, TestrpcError> {
        let timeout = timeout.unwrap_or(std::time::Duration::from_secs(60));
        let mut successes = 0;
        let mut failures = 0;

        let accounts: Vec<u32> = (0..tx_size)
            .map(|_| rand::rng().random_range(5..10) as u32)
            .collect();

        let accounts_str = accounts
            .iter()
            .map(|a| a.to_string())
            .collect::<Vec<String>>()
            .join(" ");
        for _ in 0..num_txs {
            let child = tokio::process::Command::new("sh")
                .current_dir(&self.fuzzer)
                .env("RPC_URL", rpc_url)
                .arg("-c")
                .arg(format!("npm start {accounts_str}"))
                .spawn()
                .map_err(|e| TestrpcError::RpcError(e.to_string()))?;

            tokio::select! {
                output = child.wait_with_output() => {
                    let output = output
                        .map_err(|e| TestrpcError::RpcError(e.to_string()))?;
                    if output.status.success() {
                        successes += 1;
                    } else {
                        failures += 1;
                    }
                },
                _ = tokio::time::sleep(timeout) => {
                    failures += 1;
                    tracing::debug!("EVM Adapter - send_txs to {} timed out", rpc_url);
                }
            };
        }

        Ok(RoundResults {
            sent: successes,
            failed: failures,
        })
    }
}
