use anyhow::{bail, Context};
use regex::{Captures, Regex};
use std::env;
use std::fs;
use std::path;

use serde::Deserialize;

use crate::extensions::ExtensionsConfig;
pub use rpc::*;

mod rpc;

const SUBSTRATE_CONFIG: &str = include_str!("../../rpc_configs/substrate.yml");
const ETHEREUM_CONFIG: &str = include_str!("../../rpc_configs/ethereum.yml");

#[derive(Deserialize, Debug)]
pub struct RpcDefinitionsWithBase {
    #[serde(default)]
    pub base: Option<RpcOptions>,
    #[serde(default)]
    pub methods: Vec<RpcMethod>,
    #[serde(default)]
    pub subscriptions: Vec<RpcSubscription>,
    #[serde(default)]
    pub aliases: Vec<(String, String)>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum RpcOptions {
    Path(String),
    Custom(Box<RpcDefinitionsWithBase>),
}

impl From<RpcDefinitionsWithBase> for RpcDefinitions {
    fn from(defs: RpcDefinitionsWithBase) -> Self {
        if let Some(base) = defs.base {
            let base: RpcDefinitions = base.into();

            let mut methods = base.methods;
            methods.sort_by(|a, b| a.method.cmp(&b.method));
            for m in defs.methods {
                let idx = methods.binary_search_by(|probe| probe.method.cmp(&m.method));
                match idx {
                    Ok(i) => {
                        methods[i] = m;
                    }
                    Err(i) => {
                        methods.insert(i, m);
                    }
                }
            }

            let mut subscriptions = base.subscriptions;
            subscriptions.sort_by(|a, b| a.name.cmp(&b.name));
            for s in defs.subscriptions {
                let idx = subscriptions.binary_search_by(|probe| probe.name.cmp(&s.name));
                match idx {
                    Ok(i) => {
                        subscriptions[i] = s;
                    }
                    Err(i) => {
                        subscriptions.insert(i, s);
                    }
                }
            }

            let mut aliases = base.aliases;
            aliases.sort_by(|a, b| a.0.cmp(&b.0));
            for a in defs.aliases {
                let idx = aliases.binary_search_by(|probe| probe.0.cmp(&a.0));
                match idx {
                    Ok(i) => {
                        aliases[i] = a;
                    }
                    Err(i) => {
                        aliases.insert(i, a);
                    }
                }
            }

            return RpcDefinitions {
                methods,
                subscriptions,
                aliases,
            };
        }
        RpcDefinitions {
            methods: defs.methods,
            subscriptions: defs.subscriptions,
            aliases: defs.aliases,
        }
    }
}

impl From<RpcOptions> for RpcDefinitions {
    fn from(val: RpcOptions) -> Self {
        match val {
            RpcOptions::Path(path) => match path.to_lowercase().as_str() {
                "sub" | "substrate" => serde_yaml::from_str(SUBSTRATE_CONFIG).unwrap(),
                "eth" | "ethereum" => serde_yaml::from_str(ETHEREUM_CONFIG).unwrap(),
                _ => {
                    let file = fs::File::open(path).expect("Invalid rpc config path");
                    let defs: RpcDefinitionsWithBase = serde_yaml::from_reader(file).expect("Invalid rpc config file");
                    defs.into()
                }
            },
            RpcOptions::Custom(defs) => (*defs).into(),
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct MiddlewaresConfig {
    pub methods: Vec<String>,
    pub subscriptions: Vec<String>,
}

#[derive(Debug)]
pub struct Config {
    pub extensions: ExtensionsConfig,
    pub middlewares: MiddlewaresConfig,
    pub rpcs: RpcDefinitions,
}

#[derive(Deserialize, Debug)]
pub struct ParseConfig {
    pub extensions: ExtensionsConfig,
    pub middlewares: MiddlewaresConfig,
    pub rpcs: RpcOptions,
}

impl From<ParseConfig> for Config {
    fn from(val: ParseConfig) -> Self {
        Config {
            extensions: val.extensions,
            middlewares: val.middlewares,
            rpcs: val.rpcs.into(),
        }
    }
}

// read config file specified in command line
pub fn read_config(path: impl AsRef<path::Path>) -> Result<Config, anyhow::Error> {
    let path = path.as_ref();
    let templated_config_str =
        fs::read_to_string(path).with_context(|| format!("Unable to read config file: {}", path.display()))?;

    let config_str = render_template(&templated_config_str)
        .with_context(|| format!("Unable to preprocess config file: {}", path.display()))?;

    let config: ParseConfig = serde_yaml::from_str(&config_str)
        .with_context(|| format!("Unable to parse config file: {}", path.display()))?;
    let config: Config = config.into();

    // TODO: shouldn't need to do this here. Creating a server should validates everything
    validate_config(&config)?;

    Ok(config)
}

fn render_template(templated_config_str: &str) -> Result<String, anyhow::Error> {
    // match pattern with 1 group: {variable_name}
    // match pattern with 3 groups: {variable:-word} or {variable:+word}
    // note: incompete syntax like {variable:-} will be matched since group1 is ungreedy match
    // but typically it will be rejected due to there is not corresponding env vars
    let re = Regex::new(r"\$\{([^}]+?)(?:(:-|:\+)([^}]+))?\}").unwrap();

    let mut config_str = String::with_capacity(templated_config_str.len());
    let mut last_match = 0;
    // replace pattern: with env variables
    let replacement = |caps: &Captures| -> Result<String, env::VarError> {
        match (caps.get(2), caps.get(3)) {
            (Some(sign), Some(value_default)) => {
                if sign.as_str() == ":-" {
                    env::var(&caps[1]).or(Ok(value_default.as_str().to_string()))
                } else if sign.as_str() == ":+" {
                    Ok(env::var(&caps[1]).map_or("".to_string(), |_| value_default.as_str().to_string()))
                } else {
                    Err(env::VarError::NotPresent)
                }
            }
            (None, None) => env::var(&caps[1]),
            _ => Err(env::VarError::NotPresent),
        }
    };

    // replace every matches with early return
    // when encountering error
    for caps in re.captures_iter(templated_config_str) {
        let m = caps
            .get(0)
            .expect("i==0 means implicit unnamed group that includes the entire match, which is infalliable");
        config_str.push_str(&templated_config_str[last_match..m.start()]);
        config_str.push_str(
            &replacement(&caps).with_context(|| format!("Unable to replace environment variable {}", &caps[1]))?,
        );
        last_match = m.end();
    }
    config_str.push_str(&templated_config_str[last_match..]);
    Ok(config_str)
}

fn validate_config(config: &Config) -> Result<(), anyhow::Error> {
    // TODO: validate logic should be in each individual extensions
    // validate endpoints
    for endpoint in &config.extensions.client.as_ref().unwrap().endpoints {
        if endpoint.parse::<jsonrpsee::client_transport::ws::Uri>().is_err() {
            bail!("Invalid endpoint {}", endpoint);
        }
    }

    // ensure each method has only one param with inject=true
    for method in &config.rpcs.methods {
        if method.params.iter().filter(|x| x.inject).count() > 1 {
            bail!("Method {} has more than one inject param", method.method);
        }
    }

    // ensure there is no required param after optional param
    for method in &config.rpcs.methods {
        let mut has_optional = false;
        for param in &method.params {
            if param.optional {
                has_optional = true;
            } else if has_optional {
                bail!("Method {} has required param after optional param", method.method);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_template_basically_works() {
        env::set_var("KEY", "value");
        env::set_var("ANOTHER_KEY", "another_value");
        let templated_config_str = "${KEY} some random_$tring {inside ${ANOTHER_KEY}";
        let config_str = render_template(templated_config_str).unwrap();
        assert_eq!(config_str, "value some random_$tring {inside another_value");

        env::remove_var("KEY");
        let config_str = render_template(templated_config_str);
        assert!(config_str.is_err());
        env::remove_var("ANOTHER_KEY");
    }

    #[test]
    fn render_template_supports_minus_word_syntax() {
        // ${variable:-word} indicates that if variable is set then the result will be that value. If variable is not set then word will be the result.
        env::set_var("absent_key", "value_set");
        let templated_config_str = "${absent_key:-value_default}";
        let config_str = render_template(templated_config_str).unwrap();
        assert_eq!(config_str, "value_set");
        // remove the env
        env::remove_var("absent_key");
        let config_str = render_template(templated_config_str).unwrap();
        assert_eq!(config_str, "value_default")
    }

    #[test]
    fn render_template_supports_plus_word_syntax() {
        // ${variable:+word} indicates that if variable is set then word will be the result, otherwise the result is the empty string.
        env::set_var("present_key", "any_value");
        let templated_config_str = "${present_key:+value_default}";
        let config_str = render_template(templated_config_str).unwrap();
        assert_eq!(config_str, "value_default");
        // remove the env
        env::remove_var("present_key");
        let config_str = render_template(templated_config_str).unwrap();
        assert_eq!(config_str, "")
    }

    #[test]
    fn render_template_gets_error_when_syntax_is_incomplete() {
        let templated_config_str = "${variable:-}";
        let config_str = render_template(templated_config_str);
        assert!(config_str.is_err());
        let template_config_str = "${variable:+}";
        let config_str = render_template(template_config_str);
        assert!(config_str.is_err());
    }

    #[test]
    fn read_config_with_render_template_works() {
        // It's enough to check the replacement works
        // if config itself has proper data validation
        let _config = read_config("configs/config_with_env.yml").unwrap();
    }
}
