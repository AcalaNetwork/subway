use crate::extensions::client::Client;
use crate::middlewares::{CallRequest, CallResult};
use crate::utils::errors;
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;

use super::{Extension, ExtensionRegistry};

#[derive(Default)]
pub struct Validator {
    pub config: ValidateConfig,
}

#[derive(Deserialize, Default, Debug, Clone)]
pub struct ValidateConfig {
    pub ignore_methods: Vec<String>,
}

#[async_trait]
impl Extension for Validator {
    type Config = ValidateConfig;

    async fn from_config(config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

impl Validator {
    pub fn new(config: ValidateConfig) -> Self {
        Self { config }
    }

    pub fn ignore(&self, method: &String) -> bool {
        self.config.ignore_methods.contains(method)
    }

    pub fn validate(&self, client: Arc<Client>, request: CallRequest, response: CallResult) {
        tokio::spawn(async move {
            let healthy_endpoints = client.endpoints().iter().filter(|x| x.health().score() > 0);
            futures::future::join_all(healthy_endpoints.map(|endpoint| async {
                let expected = endpoint
                    .request(
                        &request.method,
                        request.params.clone(),
                        std::time::Duration::from_secs(30),
                    )
                    .await
                    .map_err(errors::map_error);

                if response != expected {
                    let request = serde_json::to_string_pretty(&request).unwrap_or_default();
                    let actual = match &response {
                        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_default(),
                        Err(e) => e.to_string()
                    };
                    let expected = match &expected {
                        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_default(),
                        Err(e) => e.to_string()
                    };
                    let endpoint_url = endpoint.url();
                    tracing::error!("Response mismatch for request:\n{request}\nSubway response:\n{actual}\nEndpoint {endpoint_url} response:\n{expected}");
                }
            })).await;
        });
    }
}
