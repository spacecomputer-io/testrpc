use clap::Parser;
use std::{env, sync::Arc};

use testrpc::{common, config, ctx, runner, signal};

#[derive(Parser)]
struct Opts {
    #[clap(short = 'f', long, default_value = "hotshot.testrpc.yaml")]
    file: String,
    #[clap(long, default_value = "false", env = "DRY_RUN")]
    dry_run: bool,
    #[clap(long, default_value = "false")]
    gen_mock_rpcs: bool,
}

#[tokio::main]
async fn main() -> Result<(), common::TestrpcError> {
    tracing_subscriber::fmt::init(); // TODO file

    let start = std::time::Instant::now();
    tracing::info!("Starting testrpc");

    let ctx = Arc::new(ctx::Context::new());

    let opts: Opts = Opts::parse();
    if opts.dry_run {
        tracing::info!("Dry run, we will not send any RPCs");
        env::set_var("DRY_RUN", "true");
    }

    let cfg = config::load_config(opts.file.as_str()).unwrap();
    let rpc_urls = if opts.dry_run && opts.gen_mock_rpcs {
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
            return Err(common::TestrpcError::WrongNumberOfNodes(
                num_of_nodes,
                actual_num_of_nodes,
            ));
        }
    }

    let ctx_cloned = ctx.clone();
    tokio::select! {
        _ = tokio::spawn(async move {
            let round_results = runner::run(ctx_cloned, cfg.clone(), rpc_urls)
                .await
                .unwrap();
            let time_elapsed = start.elapsed();
            let results = common::FlowResults::new_from_round_results(round_results, time_elapsed);
            let results_yaml = serde_yaml::to_string(&results).unwrap();
            println!("---RESULTS--\n");
            println!("{}", results_yaml);
            println!("---END RESULTS--\n");
        }) => {}
        _ = signal::wait_exit_signals() => {
            ctx.stop();
        }
    }
    Ok(())
}
