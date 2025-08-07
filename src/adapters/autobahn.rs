/// Autobahn implementation of the adapter
use crate::adapters::Adapter;
use rand::Rng as _;
use serde_yaml::Value;
use std::collections::HashMap;
use std::{pin::Pin, future::Future};
use bytes::{BufMut, BytesMut};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use futures::sink::SinkExt as _;
use tokio::time::{interval, Duration, Instant};

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

        Ok(AutobahnArgs {
            nodes_config_file,
        })
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
            let AutobahnArgs {
                nodes_config_file,
            } = AutobahnArgs::try_from(args)?;

            // Read nodes from the config file
            let nodes = read_nodes_from_config_file(&nodes_config_file).await?;
            tracing::info!("Found {} nodes from config file.", nodes.len());
            Ok(nodes)
        })
    }

    fn send_txs(
        &self,
        tcp_endpoint: &str,
        req_id: u64,
        _iteration: u32,
        num_txs: usize,
        tx_size: usize,
    ) -> Pin<Box<dyn Future<Output = Result<RoundResults, TestrpcError>> + Send + '_>> {
        let tcp_endpoint = tcp_endpoint.to_string();
        Box::pin(async move {
            // Check for dry-run mode
            if std::env::var("DRY_RUN").is_ok() {
                tracing::info!("DRY_RUN: Would send {} transactions of {} bytes each to {}", num_txs, tx_size, tcp_endpoint);
                return Ok(RoundResults {
                    sent: num_txs,
                    failed: 0,
                });
            }

            // Validate transaction size (must be at least 9 bytes for Autobahn protocol)
            if tx_size < 9 {
                return Err(TestrpcError::RpcError(
                    "Transaction size must be at least 9 bytes for Autobahn protocol".to_string()
                ));
            }

            // Connect to the Autobahn node via TCP
            let stream = TcpStream::connect(&tcp_endpoint)
                .await
                .map_err(|e| TestrpcError::RpcError(format!("Failed to connect to {}: {}", tcp_endpoint, e)))?;

            let mut transport = Framed::new(stream, LengthDelimitedCodec::new());
            let mut successful_txs = 0;
            let mut failed_txs = 0;

            // Burst configuration following original Autobahn client logic
            const PRECISION: u64 = 20; // Sample precision
            const BURST_DURATION: u64 = 1000 / PRECISION; // 50ms bursts
            
            let burst_size = num_txs / PRECISION as usize; // Transactions per burst
            let remaining_txs = num_txs % PRECISION as usize; // Handle remainder
            
            tracing::info!("Starting burst sending to {}: {} total txs, {} per burst, {}ms intervals", 
                tcp_endpoint, num_txs, burst_size, BURST_DURATION);

            // Setup burst timing
            let mut interval_timer = interval(Duration::from_millis(BURST_DURATION));
            let mut counter = 0u64;
            let mut r: u64 = rand::rng().random(); // Random seed for unique transaction IDs
            let mut tx_index = 0;

            // NOTE: This log entry is used to compute performance
            tracing::info!("Start sending transactions to {}", tcp_endpoint);

            'main: loop {
                interval_timer.tick().await;
                let now = Instant::now();

                // Determine how many transactions to send in this burst
                let burst_txs = if counter < PRECISION - 1 {
                    burst_size
                } else {
                    // Last burst gets remaining transactions
                    burst_size + remaining_txs
                };

                // Send burst of transactions
                for x in 0..burst_txs {
                    if tx_index >= num_txs {
                        break 'main; // All transactions sent
                    }

                    let mut tx = BytesMut::with_capacity(tx_size);
                    
                    // Autobahn transaction format following original logic:
                    let (tx_type, tx_id) = if burst_size > 0 && x as u64 == counter % burst_size as u64 {
                        // Sample transaction (one per burst cycle)
                        // NOTE: This log entry is used to compute performance
                        tracing::info!("Sending sample transaction {} to {}", counter, tcp_endpoint);
                        
                        tx.put_u8(0u8); // Sample txs start with 0
                        tx.put_u64(counter); // This counter identifies the tx
                        (0u8, counter)
                    } else {
                        // Standard transactions
                        r += 1;
                        tx.put_u8(1u8); // Standard txs start with 1
                        tx.put_u64(r); // Ensures all clients send different txs
                        (1u8, r)
                    };

                    // Pad to requested size with zeros
                    tx.resize(tx_size, 0u8);
                    let bytes = tx.split().freeze();

                    // Send transaction via TCP
                    if let Err(e) = transport.send(bytes).await {
                        tracing::warn!("Failed to send transaction {} to {}: {}", tx_index, tcp_endpoint, e);
                        failed_txs += 1;
                        break 'main;
                    } else {
                        successful_txs += 1;
                        let tx_type_str = if tx_type == 0 { "SAMPLE" } else { "STANDARD" };
                        tracing::debug!("Successfully sent {} transaction #{} (ID={}) to {}", 
                            tx_type_str, tx_index + 1, tx_id, tcp_endpoint);
                    }
                    
                    tx_index += 1;
                }

                // Check if we're keeping up with the target rate
                if now.elapsed().as_millis() > BURST_DURATION as u128 {
                    // NOTE: This log entry is used to compute performance
                    tracing::warn!("Transaction rate too high for client sending to {}", tcp_endpoint);
                }

                counter += 1;
                
                // Exit if we've sent all transactions
                if tx_index >= num_txs {
                    break 'main;
                }
            }

            tracing::info!("Completed sending {} transactions to {} (success: {}, failed: {})", 
                num_txs, tcp_endpoint, successful_txs, failed_txs);

            Ok(RoundResults {
                sent: successful_txs,
                failed: failed_txs,
            })
        })
    }
}

// Read node endpoints from a JSON config file
async fn read_nodes_from_config_file(file_path: &str) -> Result<Vec<String>, TestrpcError> {
    // Read and parse the JSON config file
    // Expected format: Autobahn authorities structure with transactions endpoints
    
    let content = std::fs::read_to_string(file_path)
        .map_err(|e| TestrpcError::LoadEndpointsError(format!("Failed to read config file {}: {}", file_path, e)))?;
    
    let config: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| TestrpcError::LoadEndpointsError(format!("Failed to parse config file {}: {}", file_path, e)))?;
    
    let mut transaction_endpoints = Vec::new();
    
    // Navigate through the authorities structure
    let authorities = config.get("authorities")
        .ok_or_else(|| TestrpcError::LoadEndpointsError(
            "Config file must contain 'authorities' object".to_string()
        ))?;
    
    let authorities_obj = authorities.as_object()
        .ok_or_else(|| TestrpcError::LoadEndpointsError(
            "Expected 'authorities' to be an object".to_string()
        ))?;
    
    for (authority_key, authority_data) in authorities_obj {
        tracing::debug!("Processing authority: {}", authority_key);
        
        if let Some(workers) = authority_data.get("workers") {
            if let Some(workers_obj) = workers.as_object() {
                for (worker_id, worker_data) in workers_obj {
                    if let Some(transactions_endpoint) = worker_data.get("transactions") {
                        if let Some(endpoint_str) = transactions_endpoint.as_str() {
                            transaction_endpoints.push(endpoint_str.trim().to_string());
                            tracing::debug!("Found transactions endpoint for authority {} worker {}: {}", 
                                authority_key, worker_id, endpoint_str);
                        }
                    }
                }
            }
        }
    }
    
    if transaction_endpoints.is_empty() {
        return Err(TestrpcError::LoadEndpointsError(
            "No transaction endpoints found in config file".to_string()
        ));
    }
    
    tracing::info!("Extracted {} transaction endpoints from config", transaction_endpoints.len());
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
        args.insert("nodes_config_file".to_string(), Value::String("autobahn-nodes.json".to_string()));
        
        let parsed_args = AutobahnArgs::try_from(args).unwrap();
        assert_eq!(parsed_args.nodes_config_file, "autobahn-nodes.json");
    }

    #[test]
    fn test_autobahn_transaction_format() {
        // Test sample transaction format
        let mut tx = BytesMut::with_capacity(100);
        tx.put_u8(0u8); // Sample tx
        tx.put_u64(12345); // Counter
        tx.resize(100, 0u8);
        
        assert_eq!(tx[0], 0u8);
        assert_eq!(tx.len(), 100);
        
        // Test standard transaction format
        let mut tx2 = BytesMut::with_capacity(50);
        tx2.put_u8(1u8); // Standard tx
        tx2.put_u64(67890); // ID
        tx2.resize(50, 0u8);
        
        assert_eq!(tx2[0], 1u8);
        assert_eq!(tx2.len(), 50);
    }
}
