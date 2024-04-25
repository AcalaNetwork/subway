use garde::Validate;
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

#[derive(Deserialize, Validate, Debug)]
#[garde(allow_unvalidated)]
pub struct RpcMethod {
    pub method: String,

    #[serde(default)]
    pub cache: Option<CacheParams>,

    #[garde(custom(validate_params_with_name(&self.method)))]
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

fn validate_params_with_name(method_name: &str) -> impl FnOnce(&[MethodParam], &()) -> garde::Result + '_ {
    move |params, _| {
        // ensure each method has only one param with inject=true
        if params.iter().filter(|x| x.inject).count() > 1 {
            return Err(garde::Error::new(format!(
                "method {} has more than one inject param",
                method_name
            )));
        }
        // ensure there is no required param after optional param
        let mut has_optional = false;
        for param in params {
            if param.optional {
                has_optional = true;
            } else if has_optional {
                return Err(garde::Error::new(format!(
                    "method {} has required param after optional param",
                    method_name
                )));
            }
        }
        Ok(())
    }
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

#[derive(Deserialize, Validate, Debug)]
#[garde(allow_unvalidated)]
pub struct RpcDefinitions {
    #[garde(dive)]
    pub methods: Vec<RpcMethod>,
    #[serde(default)]
    pub subscriptions: Vec<RpcSubscription>,
    #[serde(default)]
    pub aliases: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_params_succeeds_for_valid_params() {
        let valid_params = vec![
            MethodParam {
                name: "param1".to_string(),
                ty: "u64".to_string(),
                optional: false,
                inject: false,
            },
            MethodParam {
                name: "param2".to_string(),
                ty: "u64".to_string(),
                optional: true,
                inject: false,
            },
            MethodParam {
                name: "param3".to_string(),
                ty: "u64".to_string(),
                optional: true,
                inject: false,
            },
        ];
        let method_name = "test";
        let test_fn = validate_params_with_name(method_name);
        assert!(test_fn(&valid_params, &()).is_ok());
    }

    #[test]
    fn validate_params_fails_for_more_than_one_param_has_inject_equals_true() {
        let another_invalid_params = vec![
            MethodParam {
                name: "param1".to_string(),
                ty: "u64".to_string(),
                optional: false,
                inject: true,
            },
            MethodParam {
                name: "param2".to_string(),
                ty: "u64".to_string(),
                optional: false,
                inject: true,
            },
            MethodParam {
                name: "param3".to_string(),
                ty: "u64".to_string(),
                optional: false,
                inject: true,
            },
        ];
        let method_name = "test";
        let test_fn = validate_params_with_name(method_name);
        assert!(test_fn(&another_invalid_params, &()).is_err());
    }

    #[test]
    fn validate_params_fails_for_optional_params_are_not_the_last() {
        let method_name = "test";
        let invalid_params = vec![
            MethodParam {
                name: "param1".to_string(),
                ty: "u64".to_string(),
                optional: false,
                inject: false,
            },
            MethodParam {
                name: "param2".to_string(),
                ty: "u64".to_string(),
                optional: true,
                inject: false,
            },
            MethodParam {
                name: "param3".to_string(),
                ty: "u64".to_string(),
                optional: false,
                inject: true,
            },
        ];
        let test_fn = validate_params_with_name(method_name);
        assert!(test_fn(&invalid_params, &()).is_err());
    }
}
