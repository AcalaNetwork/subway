use clap::Parser;
use serde::Deserialize;
use std::fs;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Command {
    /// The config file to use
    #[arg(short, long, default_value = "./config.yaml")]
    config: String,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    pub endpoints: Vec<String>,
    pub server: ServerConfig,
    pub rpcs: RpcDefinitions,
}

#[derive(Deserialize, Debug)]
pub struct ServerConfig {
    pub listen_address: String,
    pub port: u16,
    pub max_connections: u32,
}

#[derive(Deserialize, Debug)]
pub struct RpcMethod {
    pub method: String,

    #[serde(default)]
    pub cache: bool,

    #[serde(default)]
    pub with_block_hash: bool,

    #[serde(default)]
    pub with_block_number: bool,
}

#[derive(Deserialize, Debug)]
pub struct RpcSubscription {
    pub subscribe: String,
    pub unsubscribe: String,
    pub name: String,

    #[serde(default)]
    pub merge: bool,
}

#[derive(Deserialize, Debug)]
pub struct RpcDefinitions {
    pub methods: Vec<RpcMethod>,
    pub subscriptions: Vec<RpcSubscription>,
    pub aliases: Vec<(String, String)>,
}

pub fn read_config() -> Result<Config, String> {
    let cmd = Command::parse();

    let config =
        fs::read_to_string(cmd.config).map_err(|e| format!("Unable to read config file: {e}"))?;
    let config: Config =
        serde_yaml::from_str(&config).map_err(|e| format!("Unable to parse config file: {e}"))?;

    Ok(config)
}
