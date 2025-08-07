use serde_yaml::Value;
/// Adapter trait for implementing different RPC adapters.
/// Each adapter should implement the methods to load endpoints and send transactions.
use std::{collections::HashMap, sync::Arc, pin::Pin, future::Future};

use crate::{common, config};

pub trait Adapter {
    /// Load the RPC endpoints (peers) based on the provided arguments.
    /// This function should be implemented by each adapter to fetch the endpoints from the appropriate source.
    /// Returns a vector of RPC URLs.
    fn load_endpoints(
        &self,
        args: HashMap<String, Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, common::TestrpcError>> + Send + '_>>;

    /// Send transactions to the given RPC URL.
    /// This function should be implemented by each adapter to send transactions to the RPC URL.
    /// Returns a future that resolves to RoundResults.
    fn send_txs(
        &self,
        rpc_url: &str,
        req_id: u64,
        iteration: u32,
        num_txs: usize,
        tx_size: usize,
    ) -> Pin<Box<dyn Future<Output = Result<common::RoundResults, common::TestrpcError>> + Send + '_>>;
}

pub mod hotshot;
pub mod autobahn;

pub fn new_adapter(
    adapter_cfg: config::AdapterConfig,
) -> Result<Arc<dyn Adapter + Send + Sync>, common::TestrpcError> {
    match adapter_cfg {
        config::AdapterConfig::Hotshot => Ok(Arc::new(hotshot::HotshotAdapter::new())),
        config::AdapterConfig::Autobahn => Ok(Arc::new(autobahn::AutobahnAdapter::new())),
        _ => Err(common::TestrpcError::UnsupportedAdapter(
            adapter_cfg.to_string(),
        )),
    }
}
