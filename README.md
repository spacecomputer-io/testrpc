# testrpc

Test flows orchestration for distributed RPC testing.

## Overview

Testrpc is a tool that allows you to define a test flow in a declarative way. The flow is defined in a YAML file that describes the steps to be executed and the dependencies between them.

### Protocol Adapters

Testrpc is designed to be protocol agnostic. It uses protocol adapters to interact with the nodes. The adapter is responsible for discovering the rpcs, sending transactions, and collecting metrics.

The following adapters are available:
- [x] Hotshot
- [ ] Libp2p

Each adapter should implement the following functions:

- `load_endpoints`: Load the RPC endpoints to be used during the flow.
- `process_round`: Process a round of the flow, expected to send transactions to the RPC servers concurrently in each round


### Config File

See the following yaml defines a flow for hotshot testing:

```yaml
interval: 1 # interval between iterations (seconds)
iterations: 10 # number of iterations, none for infinite
num_of_nodes: 4 # expected number of nodes (optional, but recommended to avoid index out of range errors)
adapter: hotshot # adapter to use
args: # arguments for the adapter
  coordinator_url: http://127.0.0.1:3030
# rpcs: # rpcs to use, if not defined, the adapter will load them from the coordinator
#   - http://localhost:5000
#   - http://localhost:5001
#   - http://localhost:5002
#   - http://localhost:5003
round_templates: # reusable round templates
  10_txs:
    txs: 10 # number of transactions to send
    tx_size: 100 # size of each transaction
rounds: # rounds to run continuously, each round will be an iteration
  - rpcs: [1,2] # rpcs to use out of the available ones
    use_template: 10_txs # use a round template
  - rpcs: [3,0]
    use_template: 10_txs
  - rpcs: [1,0]
    template: # define a round template inline
        txs: 2
        tx_size: 200
```

#### Generating Config Files

To generate a config file from a template, you can use the `tmpl.py` script:

```bash
python3 ./scripts/tmpl.py ./tmpl/hotshot.testrpc.yaml.j2 ./tmpl/values/hotshot.yaml --num-nodes 10
```

## Usage


### Install on OS

You can install the binary on your system with:

```bash
make install
# cargo install --locked --path .
```

Or directly from git:

```bash
cargo install --git https://github.com/spacecoinxyz/testrpc --branch main --locked testrpc
```

Now you can run the binary with the path to the config file:

```bash
testrpc -f my.testrpc.yaml
```

### Build from source

Run with cargo, build the project and run the binary with the path to the config file.
Use `RUST_LOG` to control the verbosity of the logs:

```bash
cargo build
RUST_LOG=debug ./target/debug/testrpc -f my.testrpc.yaml
```

Or directly with cargo:

```bash
RUST_LOG=debug cargo run --bin testrpc -- -f my.testrpc.yaml
```

### Dry run

You can run a dry run to see the steps that would be executed, without actually making RPC calls:

```bash
RUST_LOG=debug testrpc -f $PWD/examples/hotshot.testrpc.yaml --dry-run
```

### Development

Run the tests with:

```bash
cargo test
```

Make sure to run fmt and clippy before pushing, they will fail the CI if not passing:

```bash
cargo fmt
cargo clippy
```

## License

This project is licensed under the MIT License. See the [LICENSE](./LICENSE) file for details.
