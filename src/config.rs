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
    pub merge_subscription_keep_alive_seconds: Option<u64>,
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

    if let Ok(endpoints) = std::env::var("ENDPOINTS") {
        log::info!("Override endpoints with env.ENDPOINTS");
        config.endpoints = endpoints
            .split(',')
            .map(|x| x.trim().to_string())
            .collect::<Vec<_>>();
    }

    if let Ok(env_port) = std::env::var("PORT") {
        log::info!("Override port with env.PORT");
        let port = env_port.parse::<u16>();
        if let Ok(port) = port {
            config.server.port = port;
        }
    }

    validate_config(&config);

    Ok(config)
}

pub fn validate_config(config: &Config) {
    // validate endpoints
    assert!(
        config
            .endpoints
            .iter()
            .all(|x| x.parse::<jsonrpsee::client_transport::ws::Uri>().is_ok()),
        "Invalid endpoint"
    );

    // ensure each method has only one param with inject=true
    for method in &config.rpcs.methods {
        assert!(
            method.params.iter().filter(|x| x.inject).count() <= 1,
            "Method {} has more than one inject param",
            method.method
        );
    }

    // ensure there is no required param after optional param
    for method in &config.rpcs.methods {
        let mut has_optional = false;
        for param in &method.params {
            if param.optional {
                has_optional = true;
            } else {
                assert!(
                    !has_optional,
                    "Method {} has required param after optional param",
                    method.method
                );
            }
        }
    }
}
