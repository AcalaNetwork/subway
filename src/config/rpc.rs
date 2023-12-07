use jsonrpsee::core::JsonValue;
use serde::Deserialize;

#[derive(Clone, Deserialize, Debug, Eq, PartialEq)]
pub struct CacheParams {
    #[serde(default)]
    pub size: Option<usize>,
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
}

#[derive(Clone, Deserialize, Debug, Eq, PartialEq)]
pub struct MethodParam {
    pub name: String,
    #[serde(default)]
    pub ty: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub inject: bool,
}

#[derive(Deserialize, Debug)]
pub struct RpcMethod {
    pub method: String,

    #[serde(default)]
    pub cache: Option<CacheParams>,

    #[serde(default)]
    pub params: Vec<MethodParam>,

    #[serde(default)]
    pub response: Option<JsonValue>,

    #[serde(default)]
    pub delay_ms: Option<u64>,

    /// This should not exceed max cell capacity. If it does,
    /// method will return error. Burst size is the max cell capacity.
    /// If rate limit is not configured, this will be ignored.
    /// e.g. if rate limit is configured as 10r per 2s and rate_limit_weight is 10,
    /// then only 1 call is allowed per 2s. If rate_limit_weight is 5, then 2 calls
    /// are allowed per 2s. If rate_limit_weight is greater than 10, then method will
    /// return error "rate limit exceeded".
    /// Add this if you want to modify the default value of 1.
    #[serde(default = "default_rate_limit_weight")]
    pub rate_limit_weight: u32,
}

fn default_rate_limit_weight() -> u32 {
    1
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
    #[serde(default)]
    pub subscriptions: Vec<RpcSubscription>,
    #[serde(default)]
    pub aliases: Vec<(String, String)>,
}
