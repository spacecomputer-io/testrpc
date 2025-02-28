use thiserror::Error;

#[derive(Error, Debug)]
pub enum TestflowError {
    #[error("Num of nodes mismatch: expected {0}, got {1}")]
    WrongNumberOfNodes(usize, usize),
    #[error("Unsupported adapter: {0}")]
    UnsupportedAdapter(String),
    #[error("Failed to load config: {0}")]
    LoadConfigError(String),
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

#[derive(Debug, Clone)]
pub struct RoundResults {
    pub sent: usize,
    pub failed: usize,
}

pub struct FlowContext {
    pub iteration: u32,
    pub rpc_urls: Vec<String>,
    pub req_id: u64,
}
