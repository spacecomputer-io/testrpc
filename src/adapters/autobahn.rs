/// Autobahn implementation of the adapter.
///
/// Autobahn does not use the trait-based per-round `send_txs` flow — it runs
/// exclusively in continuous streaming mode driven by `runner::run_continuous_mode`,
/// which writes length-delimited TCP frames directly. Only `load_endpoints` is
/// meaningful here.
use crate::adapters::Adapter;
use serde_yaml::Value;
use std::collections::HashMap;
use std::{future::Future, pin::Pin};

use crate::common::{RoundResults, TestrpcError};

/// Arguments for the Autobahn adapter
pub struct AutobahnArgs {
    /// Path to JSON file containing Autobahn node endpoints (IP:port format)
    pub nodes_config_file: String,
}

impl TryFrom<HashMap<String, Value>> for AutobahnArgs {
    type Error = TestrpcError;

    fn try_from(args: HashMap<String, Value>) -> Result<Self, Self::Error> {
        let nodes_config_file = match args.get("nodes_config_file") {
            Some(Value::String(file_path)) => file_path.clone(),
            _ => return Err(TestrpcError::MissingArgs("nodes_config_file".to_string())),
        };

        Ok(AutobahnArgs { nodes_config_file })
    }
}

pub struct AutobahnAdapter;

impl AutobahnAdapter {
    pub fn new() -> Self {
        AutobahnAdapter {}
    }
}

impl Default for AutobahnAdapter {
    fn default() -> Self {
        AutobahnAdapter::new()
    }
}

impl Adapter for AutobahnAdapter {
    fn load_endpoints(
        &self,
        args: HashMap<String, Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, TestrpcError>> + Send + '_>> {
        Box::pin(async move {
            let AutobahnArgs { nodes_config_file } = AutobahnArgs::try_from(args)?;

            // Read nodes from the config file
            let nodes = read_nodes_from_config_file(&nodes_config_file).await?;
            tracing::info!("Found {} nodes from config file.", nodes.len());
            Ok(nodes)
        })
    }

    fn send_txs(
        &self,
        _tcp_endpoint: &str,
        _req_id: u64,
        _iteration: u32,
        _num_txs: usize,
        _tx_size: usize,
    ) -> Pin<Box<dyn Future<Output = Result<RoundResults, TestrpcError>> + Send + '_>> {
        // Autobahn runs only via continuous streaming mode (load_stages); the runner
        // routes that path without going through the Adapter trait, so this method
        // is unreachable in normal operation. The dispatcher in `runner::run` rejects
        // an autobahn config without `load_stages` before we get here.
        Box::pin(async move {
            Err(TestrpcError::RpcError(
                "autobahn adapter does not support legacy batch send_txs — \
                 use continuous mode with load_stages"
                    .to_string(),
            ))
        })
    }
}

// Read node endpoints from a JSON config file
async fn read_nodes_from_config_file(file_path: &str) -> Result<Vec<String>, TestrpcError> {
    // Read and parse the JSON config file
    // Expected format: Autobahn authorities structure with transactions endpoints

    let content = std::fs::read_to_string(file_path).map_err(|e| {
        TestrpcError::LoadEndpointsError(format!("Failed to read config file {}: {}", file_path, e))
    })?;

    let config: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        TestrpcError::LoadEndpointsError(format!(
            "Failed to parse config file {}: {}",
            file_path, e
        ))
    })?;

    let mut transaction_endpoints = Vec::new();

    // Navigate through the authorities structure
    let authorities = config.get("authorities").ok_or_else(|| {
        TestrpcError::LoadEndpointsError(
            "Config file must contain 'authorities' object".to_string(),
        )
    })?;

    let authorities_obj = authorities.as_object().ok_or_else(|| {
        TestrpcError::LoadEndpointsError("Expected 'authorities' to be an object".to_string())
    })?;

    for (authority_key, authority_data) in authorities_obj {
        tracing::debug!("Processing authority: {}", authority_key);

        if let Some(workers) = authority_data.get("workers") {
            if let Some(workers_obj) = workers.as_object() {
                for (worker_id, worker_data) in workers_obj {
                    if let Some(transactions_endpoint) = worker_data.get("transactions") {
                        if let Some(endpoint_str) = transactions_endpoint.as_str() {
                            transaction_endpoints.push(endpoint_str.trim().to_string());
                            tracing::debug!(
                                "Found transactions endpoint for authority {} worker {}: {}",
                                authority_key,
                                worker_id,
                                endpoint_str
                            );
                        }
                    }
                }
            }
        }
    }

    if transaction_endpoints.is_empty() {
        return Err(TestrpcError::LoadEndpointsError(
            "No transaction endpoints found in config file".to_string(),
        ));
    }

    tracing::info!(
        "Extracted {} transaction endpoints from config",
        transaction_endpoints.len()
    );
    for (i, endpoint) in transaction_endpoints.iter().enumerate() {
        tracing::debug!("Endpoint {}: {}", i, endpoint);
    }

    Ok(transaction_endpoints)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autobahn_args_parsing() {
        let mut args = HashMap::new();
        args.insert(
            "nodes_config_file".to_string(),
            Value::String("autobahn-nodes.json".to_string()),
        );

        let parsed_args = AutobahnArgs::try_from(args).unwrap();
        assert_eq!(parsed_args.nodes_config_file, "autobahn-nodes.json");
    }
}
