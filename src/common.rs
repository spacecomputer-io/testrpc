use serde::{Deserialize, Serialize};
use std::{future::Future, time::Duration, pin::Pin};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TestrpcError {
    #[error("Num of nodes mismatch: expected {0}, got {1}")]
    WrongNumberOfNodes(usize, usize),
    #[error("Unsupported adapter: {0}")]
    UnsupportedAdapter(String),
    #[error("Failed to load config (file: {1}): {0}")]
    LoadConfigError(String, String),
    #[error("Missing arguments: {0}")]
    MissingArgs(String),
    #[error("Failed to load endpoints: {0}")]
    LoadEndpointsError(String),
    #[error("Failed to load round template: {0}")]
    LoadRoundTemplateError(String),
    #[error("RPC error: {0}")]
    RpcError(String),
    #[error("Execution error: {0}")]
    ExecutionError(String),
    #[error("Termination error: {0}")]
    TerminationError(String),
    #[error("Join error: {0}")]
    JoinError(#[from] tokio::task::JoinError),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RoundResults {
    pub sent: usize,
    pub failed: usize,
    // TODO: bytes
    // pub bytes_sent: usize,
    // pub bytes_failed: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FlowResults {
    pub rounds: Vec<RoundResults>,
    pub total: RoundResults,
    pub total_time: Duration,
    pub total_iterations: u32,
}

impl FlowResults {
    pub fn new_from_round_results(rounds: Vec<RoundResults>, total_time: Duration) -> Self {
        let total_iterations = rounds.len() as u32;
        let mut total = RoundResults { sent: 0, failed: 0 };
        for round in rounds.iter() {
            total.sent += round.sent;
            total.failed += round.failed;
        }
        Self {
            rounds,
            total,
            total_time,
            total_iterations,
        }
    }
}

/// Retry a given operation a specified number of times with a delay between attempts 
/// and optional exponential backoff.
/// Returns Ok(()) if successful, or an error if all retries fail.
pub async fn retry<T: Send + 'static>(
    retries: usize,
    delay: Duration,
    mut operation: impl FnMut() -> Pin<Box<dyn Future<Output = Result<T, TestrpcError>> + Send>>,
    exponential_backoff: bool,
) -> Result<T, TestrpcError> {
    let mut delay = delay;
    for _ in 0..retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                tracing::warn!("Operation failed: {}. Retrying in {:?}...", e, delay);
                tokio::time::sleep(delay).await;
                if exponential_backoff {
                    delay *= 2;
                }
            }
        }
    }
    Err(TestrpcError::ExecutionError("Max retries reached".to_string()))
}