use clap::Parser;
use serde::Deserialize;
use std::fs;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Command {
    /// The config file to use
    #[arg(short, long, default_value = "./config.yml")]
    config: String,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    pub endpoints: Vec<String>,
    pub stale_timeout_seconds: u64,
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
    pub params: Vec<MethodParam>,

    #[serde(default)]
    pub cache: usize,
}

#[derive(Clone, Deserialize, Debug, Eq, PartialEq)]
pub struct MethodParam {
    pub name: String,
    pub ty: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub inject: bool,
}

#[derive(Copy, Clone, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    // Replace old value with new value
    Replace,
    // Merge old storage changes with new changes
    MergeStorageChanges,
}

#[derive(Deserialize, Debug)]
pub struct RpcSubscription {
    pub subscribe: String,
    pub unsubscribe: String,
    pub name: String,

    #[serde(default)]
    pub merge_strategy: Option<MergeStrategy>,
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
    let mut config: Config =
        serde_yaml::from_str(&config).map_err(|e| format!("Unable to parse config file: {e}"))?;

    let env_port = std::env::var("PORT");
    if let Ok(env_port) = env_port {
        let port = env_port.parse::<u16>();
        if let Ok(port) = port {
            config.server.port = port;
        }
    }

    Ok(config)
}
