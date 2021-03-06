// Built-in deps
use std::env;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;
// External uses
use futures::{channel::mpsc, executor::block_on, SinkExt};
use web3::types::{H160, H256};
// Local uses
use crate::params::block_chunk_sizes;
use url::Url;

/// If its placed inside thread::spawn closure it will notify channel when this thread panics.
pub struct ThreadPanicNotify(pub mpsc::Sender<bool>);

impl Drop for ThreadPanicNotify {
    fn drop(&mut self) {
        if std::thread::panicking() {
            block_on(self.0.send(true)).unwrap();
        }
    }
}

/// Obtains the environment variable value.
/// Panics if there is no environment variable with provided name set.
pub fn get_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|e| panic!("Env var {} missing, {}", name, e))
}

/// Obtains the environment variable value and parses it using the `FromStr` type implementation.
/// Panics if there is no environment variable with provided name set, or the value cannot be parsed.
pub fn parse_env<F>(name: &str) -> F
where
    F: FromStr,
    F::Err: std::fmt::Debug,
{
    get_env(name)
        .parse()
        .unwrap_or_else(|e| panic!("Failed to parse environment variable {}: {:?}", name, e))
}

/// Similar to `parse_env`, but also takes a function to change the variable value before parsing.
pub fn parse_env_with<T, F>(name: &str, f: F) -> T
where
    T: FromStr,
    T::Err: std::fmt::Debug,
    F: FnOnce(&str) -> &str,
{
    let env_var = get_env(name);

    f(&env_var)
        .parse()
        .unwrap_or_else(|e| panic!("Failed to parse environment variable {}: {:?}", name, e))
}

/// Configuration options for `eth_sender`.
#[derive(Debug, Clone)]
pub struct EthSenderOptions {
    pub expected_wait_time_block: u64,
    pub tx_poll_period: Duration,
    pub wait_confirmations: u64,
    pub max_txs_in_flight: u64,
    pub is_enabled: bool,
}

impl EthSenderOptions {
    /// Parses the `eth_sender` configuration options values from the environment variables.
    /// Panics if any of options is missing or has inappropriate value.
    pub fn from_env() -> Self {
        let tx_poll_period_secs: u64 = parse_env("ETH_TX_POLL_PERIOD");

        Self {
            expected_wait_time_block: parse_env("ETH_EXPECTED_WAIT_TIME_BLOCK"),
            tx_poll_period: Duration::new(tx_poll_period_secs, 0),
            wait_confirmations: parse_env("ETH_WAIT_CONFIRMATIONS"),
            max_txs_in_flight: parse_env("ETH_MAX_TXS_IN_FLIGHT"),
            is_enabled: parse_env("ETH_IS_ENABLED"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProverOptions {
    pub prepare_data_interval: Duration,
    pub heartbeat_interval: Duration,
    pub cycle_wait: Duration,
    pub gone_timeout: Duration,
}

impl ProverOptions {
    /// Parses the configuration options values from the environment variables.
    /// Panics if any of options is missing or has inappropriate value.
    pub fn from_env() -> Self {
        let prepare_data_interval =
            Duration::from_millis(parse_env("PROVER_PREPARE_DATA_INTERVAL"));
        let heartbeat_interval = Duration::from_millis(parse_env("PROVER_HEARTBEAT_INTERVAL"));
        let cycle_wait = Duration::from_millis(parse_env("PROVER_CYCLE_WAIT"));
        let gone_timeout = Duration::from_millis(parse_env("PROVER_GONE_TIMEOUT"));

        Self {
            prepare_data_interval,
            heartbeat_interval,
            cycle_wait,
            gone_timeout,
        }
    }
}

/// Configuration options for `admin server`.
#[derive(Debug, Clone)]
pub struct AdminServerOptions {
    pub admin_http_server_url: Url,
    pub admin_http_server_address: SocketAddr,
    pub secret_auth: String,
}

impl AdminServerOptions {
    /// Parses the configuration options values from the environment variables.
    /// Panics if any of options is missing or has inappropriate value.
    pub fn from_env() -> Self {
        Self {
            admin_http_server_url: parse_env("ADMIN_SERVER_API_URL"),
            admin_http_server_address: parse_env("ADMIN_SERVER_API_BIND"),
            secret_auth: parse_env("SECRET_AUTH"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum TokenPriceSource {
    CoinMarketCap { base_url: Url },
    CoinGecko { base_url: Url },
}

impl TokenPriceSource {
    fn from_env() -> Self {
        match get_env("TOKEN_PRICE_SOURCE").to_lowercase().as_str() {
            "coinmarketcap" => Self::CoinMarketCap {
                base_url: parse_env("COINMARKETCAP_BASE_URL"),
            },
            "coingecko" => Self::CoinGecko {
                base_url: parse_env("COINGECKO_BASE_URL"),
            },
            source => panic!("Unknown token price source: {}", source),
        }
    }
}

/// Configuration options related to generating blocks by state keeper.
/// Each block is generated after a certain amount of miniblock iterations.
/// Miniblock iteration is a routine of processing transactions received so far.
#[derive(Debug, Clone)]
pub struct MiniblockTimings {
    /// Miniblock iteration interval.
    pub miniblock_iteration_interval: Duration,
    /// Max number of miniblocks (produced every period of `TX_MINIBATCH_CREATE_TIME`) if one block.
    pub max_miniblock_iterations: usize,
    /// Max number of miniblocks for block with fast withdraw operations (defaults to `max_minblock_iterations`).
    pub fast_miniblock_iterations: usize,
}

impl MiniblockTimings {
    pub fn from_env() -> Self {
        let fast_miniblock_iterations = if env::var("FAST_BLOCK_MINIBLOCKS_ITERATIONS").is_ok() {
            parse_env("FAST_BLOCK_MINIBLOCKS_ITERATIONS")
        } else {
            parse_env("MINIBLOCKS_ITERATIONS")
        };

        Self {
            miniblock_iteration_interval: Duration::from_millis(parse_env::<u64>(
                "MINIBLOCK_ITERATION_INTERVAL",
            )),
            max_miniblock_iterations: parse_env("MINIBLOCKS_ITERATIONS"),
            fast_miniblock_iterations,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigurationOptions {
    pub rest_api_server_address: SocketAddr,
    pub json_rpc_http_server_address: SocketAddr,
    pub json_rpc_ws_server_address: SocketAddr,
    pub web3_url: String,
    pub genesis_tx_hash: H256,
    pub contract_eth_addr: H160,
    pub governance_eth_addr: H160,
    pub operator_fee_eth_addr: H160,
    pub operator_commit_eth_addr: H160,
    pub operator_private_key: Option<H256>,
    pub chain_id: u8,
    pub gas_price_factor: f64,
    pub prover_server_address: SocketAddr,
    pub confirmations_for_eth_event: u64,
    pub api_requests_caches_size: usize,
    pub available_block_chunk_sizes: Vec<usize>,
    pub max_number_of_withdrawals_per_block: usize,
    pub eth_watch_poll_interval: Duration,
    pub eth_network: String,
    pub idle_provers: u32,
    pub miniblock_timings: MiniblockTimings,
    pub prometheus_export_port: u16,
    pub token_price_source: TokenPriceSource,
    pub witness_generators: usize,
    /// Fee increase coefficient for fast processing of withdrawal.
    pub ticker_fast_processing_coeff: f64,
}

impl ConfigurationOptions {
    /// Parses the configuration options values from the environment variables.
    /// Panics if any of options is missing or has inappropriate value.
    pub fn from_env() -> Self {
        let mut available_block_chunk_sizes = block_chunk_sizes().to_vec();
        available_block_chunk_sizes.sort();

        Self {
            rest_api_server_address: parse_env("REST_API_BIND"),
            json_rpc_http_server_address: parse_env("HTTP_RPC_API_BIND"),
            json_rpc_ws_server_address: parse_env("WS_API_BIND"),
            web3_url: get_env("WEB3_URL"),
            genesis_tx_hash: parse_env_with("GENESIS_TX_HASH", |s| &s[2..]),
            contract_eth_addr: parse_env_with("CONTRACT_ADDR", |s| &s[2..]),
            governance_eth_addr: parse_env_with("GOVERNANCE_ADDR", |s| &s[2..]),
            operator_commit_eth_addr: parse_env_with("OPERATOR_COMMIT_ETH_ADDRESS", |s| &s[2..]),
            operator_fee_eth_addr: parse_env_with("OPERATOR_FEE_ETH_ADDRESS", |s| &s[2..]),
            operator_private_key: if env::var("OPERATOR_PRIVATE_KEY").is_ok() {
                Some(parse_env("OPERATOR_PRIVATE_KEY"))
            } else {
                None
            },
            chain_id: parse_env("CHAIN_ID"),
            gas_price_factor: parse_env("GAS_PRICE_FACTOR"),
            prover_server_address: parse_env("PROVER_SERVER_BIND"),
            confirmations_for_eth_event: parse_env("CONFIRMATIONS_FOR_ETH_EVENT"),
            api_requests_caches_size: parse_env("API_REQUESTS_CACHES_SIZE"),
            available_block_chunk_sizes,
            max_number_of_withdrawals_per_block: parse_env("MAX_NUMBER_OF_WITHDRAWALS_PER_BLOCK"),
            eth_watch_poll_interval: Duration::from_millis(parse_env::<u64>(
                "ETH_WATCH_POLL_INTERVAL",
            )),
            eth_network: parse_env("ETH_NETWORK"),
            idle_provers: parse_env("IDLE_PROVERS"),
            miniblock_timings: MiniblockTimings::from_env(),
            prometheus_export_port: parse_env("PROMETHEUS_EXPORT_PORT"),
            token_price_source: TokenPriceSource::from_env(),
            witness_generators: parse_env("WITNESS_GENERATORS"),
            ticker_fast_processing_coeff: parse_env("TICKER_FAST_PROCESSING_COEFF"),
        }
    }
}

/// Possible block chunks sizes and corresponding setup powers of two,
/// this is only parameters needed to create verifying contract.
#[derive(Debug)]
pub struct AvailableBlockSizesConfig {
    pub blocks_chunks: Vec<usize>,
    pub blocks_setup_power2: Vec<u32>,
}

impl AvailableBlockSizesConfig {
    pub fn from_env() -> Self {
        let result = Self {
            blocks_chunks: get_env("SUPPORTED_BLOCK_CHUNKS_SIZES")
                .split(',')
                .map(|p| p.parse().unwrap())
                .collect(),
            blocks_setup_power2: get_env("SUPPORTED_BLOCK_CHUNKS_SIZES_SETUP_POWERS")
                .split(',')
                .map(|p| p.parse().unwrap())
                .collect(),
        };
        assert_eq!(
            result.blocks_chunks.len(),
            result.blocks_setup_power2.len(),
            "block sized and setup powers should have same length, check config file"
        );
        result
    }
}
