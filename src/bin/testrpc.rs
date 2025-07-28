use clap::Parser;
use std::{env, sync::Arc};

use testrpc::{common, config, ctx, logging, runner, signal};

#[derive(Parser, Debug, Clone)]
struct Opts {
    #[clap(short = 'f', long, default_value = "hotshot.testrpc.yaml")]
    file: String,
    #[clap(long, default_value = "false", env = "DRY_RUN")]
    dry_run: bool,
    #[clap(long, default_value = "false")]
    gen_mock_rpcs: bool,
    #[clap(long)]
    log_file: Option<String>,
    #[clap(long, default_value = "debug")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<(), common::TestrpcError> {
    let opts: Opts = Opts::parse();
    if let Some(log_file) = opts.log_file {
        env::set_var("RUST_LOG_FILE", log_file.clone());
        println!("Using log file: {}", log_file.clone());
    } else {
        println!("Output log to stdout");
    }
    env::set_var("RUST_LOG", opts.log_level.clone());
    println!("Using log level: {}", &opts.log_level);

    let _log_guard = logging::initialize_logging();
    let ctx = Arc::new(ctx::Context::new());
    let start = std::time::Instant::now();

    tracing::info!("Starting testrpc with config file: {}", &opts.file);

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
            println!("{results_yaml}");
            println!("---END RESULTS--\n");
        }) => {}
        _ = signal::wait_exit_signals() => {
            ctx.stop();
        }
    }
    Ok(())
}
