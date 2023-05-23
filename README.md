## ------------------------------------
## Tests and Docs are still being written 🏗️.
Tests are still being written, assume bugs until tested. If you would like to help contribute on the tests or docs, feel free to open up an issue or make a PR.
## ------------------------------------

# cfmms-rs

`cfmms-rs` is a vertically integrated library for interacting with CFMMs (constant function market makers) on Ethereum. Features include:
- batch sync token pair data from Uniswap V2 and V3 through optimized smart contracts that minimize the number of RPC calls
- calculate token pair spot prices and simulate token swaps
- execute transactions 
Sync pairs simulate swaps, and interact with constant function market makers on Ethereum.

- [Crates.io](https://crates.io/crates/cfmms)
- [Documentation in progress](https://docs.rs/cfmms/0.1.3/cfmms/)

## Project Layout
```./
├── src/
│   ├── batch_requests/
        |── uniswap_v2/
        |── uniswap_v3/
│   ├── dex/
        |── mod.rs
        |── uniswap_v2.rs
        |── uniswap_v3.rs
│   ├── pool/
        |── mod.rs
        |── uniswap_v2.rs
        |── uniswap_v3.rs
│   ├── abi.rs
│   ├── checkpoint.rs
│   ├── errors.rs
│   ├── lib.rs
│   ├── sync.rs
│   └── throttle.rs
├── Cargo.lock
├── Cargo.toml
├── foundry.toml
└── README.md
```

The core logic is contained in the following files:
* batch_requests - handles batch pool requests to the Ethereum endpoint using solidity contracts in `contracts/`
* dex - contains interfaces that handle dex invariants
* pool - contains pool interfaces that handle pools based on dex invariants
* abi - generates bindings for the UniswapV2 and UniswapV3 contracts
* sync - syncs multiple pool states between all dexes


## Supported Dexes

| Dex | Status |
|----------|------|
| UniswapV2 variants  | ✅||
| UniswapV3  | ✅||

## Build, Run Tests, and Examples
1. In order to build, clone the github repo:
`git clone https://github.com/paradigmxyz/artemis
cd artemis`

2. Run tests with cargo `cargo test --all`

3. To run any of the examples, first set a local environment variable called `ETHEREUM_MAINNET_ENDPOINT`. Then you can simply run `cargo run --example <example_name>`.

