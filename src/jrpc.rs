use std::env;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::common::TestrpcError;

/// RPC request structure
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RpcRequest {
    /// JSON-RPC version
    jsonrpc: String,
    /// RPC method
    method: String,
    /// RPC parameters
    params: Value,
    /// RPC request ID
    id: u64,
}

/// RPC response structure
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RpcResponse {
    /// JSON-RPC version
    jsonrpc: String,
    /// RPC result
    result: Value,
    /// RPC request ID
    id: u64,
}

pub async fn send_noop(
    rpc_url: &str,
    rpc_request: RpcRequest,
) -> Result<RpcResponse, TestrpcError> {
    let as_json = serde_json::to_string(&rpc_request)
        .map_err(|e| TestrpcError::RpcError(format!("Failed to serialize request: {e}")))?;
    tracing::info!(
        "Sending noop request with {} bytes to {}",
        as_json.len(),
        rpc_url
    );
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    Ok(RpcResponse {
        jsonrpc: "2.0".to_string(),
        result: serde_json::json!({}),
        id: rpc_request.id,
    })
}

/// Sends requests to the RPC server
pub async fn send(
    rpc_url: &str,
    req_id: u64,
    method: &str,
    params: Value,
) -> Result<RpcResponse, TestrpcError> {
    let rpc_request = RpcRequest {
        jsonrpc: "2.0".to_string(),
        method: method.to_string(),
        params,
        id: req_id,
    };
    if env::var("DRY_RUN").is_ok() {
        return send_noop(rpc_url, rpc_request).await;
    }
    let client = reqwest::Client::new();

    let start_time = std::time::Instant::now();

    let response = client
        .post(format!("http://{rpc_url}"))
        .json(&rpc_request)
        .send()
        .await
        .map_err(|e| TestrpcError::RpcError(format!("Failed to make request: {e}")))?;

    tracing::info!(
        "Got RPC response after {}ms",
        start_time.elapsed().as_millis()
    );

    let response: RpcResponse = response
        .json()
        .await
        .map_err(|e| TestrpcError::RpcError(format!("Failed to parse response: {e}")))?;

    tracing::debug!("RPC response: {:?}", response);

    Ok(response)
}
