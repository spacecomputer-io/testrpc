pub mod common;
pub mod config;
pub mod ctx;
pub mod hotshot;
pub mod jrpc;
pub mod logging;
pub mod runner;
pub mod signal;

#[cfg(test)]
mod test {
    use std::{env, sync::Arc};

    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_e2e_dry_run() {
        env::set_var("DRY_RUN", "true"); // run in dry-run mode
        let raw_cfg_yaml: &str = r#"
interval: 1
iterations: 4
num_of_nodes: 4
adapter: hotshot
args: # arguments for the adapter
  coordinator_url: http://127.0.0.1:3030
round_templates:
  10_txs:
    txs: 10
    tx_size: 100
rpcs:
    - http://localhost:5000
    - http://localhost:5001
    - http://localhost:5002
    - http://localhost:5003
rounds:
  - rpcs: [3,0]
    use_template: 10_txs
  - rpcs: [1,2]
    template:
        txs: 10
        tx_size: 1000
        "#;
        let cfg = config::parse_config_yaml(raw_cfg_yaml).unwrap();
        let ctx = Arc::new(ctx::Context::new());
        let rpc_urls = runner::load_endpoints(cfg.clone()).await.unwrap();
        let ctx_cloned = ctx.clone();
        let handle = tokio::spawn(async move {
            // wait for the test to complete
            // use a timeout of <interval> * <interations> + buffer
            let buffer = 1;
            let timeout = 1 * 4 + buffer;
            sleep(Duration::from_secs(timeout)).await;
            ctx_cloned.stop();
        });
        tokio::select! {
            _ = handle => {
                panic!("Timed out w/o completion");
            }
            Ok(results) = runner::run(ctx, cfg, rpc_urls) => {
                assert_eq!(results.len(), 4);
                for result in results {
                    assert_eq!(result.sent, 20);
                    assert_eq!(result.failed, 0);
                }
            }
        };
    }
}
