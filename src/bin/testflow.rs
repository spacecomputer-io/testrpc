use clap::Parser;
use std::{env, sync::Arc};

use testflow::{common, config, ctx, runner, signal};

#[derive(Parser)]
struct Opts {
    #[clap(short = 'f', long, default_value = "hotshot.testflow.yaml")]
    file: String,
    #[clap(long, default_value = "false", env = "DRY_RUN")]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<(), common::TestflowError> {
    tracing_subscriber::fmt::init(); // TODO file

    let ctx = Arc::new(ctx::Context::new());

    let opts: Opts = Opts::parse();
    if opts.dry_run {
        tracing::info!("Dry run, we will not send any RPCs");
        env::set_var("DRY_RUN", "true");
    }

    let cfg = config::load_config(opts.file.as_str()).unwrap();
    let rpc_urls = if opts.dry_run {
        let num_of_nodes = cfg.num_of_nodes.unwrap_or(4);
        let mut urls = Vec::new();
        for i in 0..num_of_nodes {
            urls.push(format!("http://dummy:{}", 5000 + i));
        }
        urls
    } else {
        runner::load_endpoints(cfg.clone()).await.unwrap()
    };

    if let Some(num_of_nodes) = cfg.num_of_nodes {
        let actual_num_of_nodes = rpc_urls.len();
        if actual_num_of_nodes != num_of_nodes {
            return Err(common::TestflowError::WrongNumberOfNodes(
                num_of_nodes,
                actual_num_of_nodes,
            ));
        }
    }

    let ctx_cloned = ctx.clone();
    tokio::select! {
        _ = tokio::spawn(async move {
            let results = runner::run(ctx_cloned, cfg.clone(), rpc_urls)
                .await
                .unwrap();
            println!("---RESULTS--\n{:?}\n--END_RESULTS--\n", results);
        }) => {}
        _ = signal::wait_exit_signals() => {
            ctx.stop();
        }
    }
    Ok(())
}
