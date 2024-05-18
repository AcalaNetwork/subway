use crate::extensions::client::Client;
use crate::middlewares::{CallRequest, CallResult};
use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;

use super::{Extension, ExtensionRegistry};

pub struct Validator {
    config: ValidateConfig,
    clients: Vec<Arc<Client>>,
}

#[derive(Deserialize, Default, Debug, Clone)]
pub struct ValidateConfig {
    pub ignore_methods: Vec<String>,
}

#[async_trait]
impl Extension for Validator {
    type Config = ValidateConfig;

    async fn from_config(config: &Self::Config, registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        let client = registry.get::<Client>().await.expect("Client extension not found");

        let clients = client
            .endpoints()
            .iter()
            .map(|e| Arc::new(Client::with_endpoints([e]).expect("Unable to create client")))
            .collect();

        Ok(Self::new(config.clone(), clients))
    }
}

impl Validator {
    pub fn new(config: ValidateConfig, clients: Vec<Arc<Client>>) -> Self {
        Self { config, clients }
    }

    pub fn ignore(&self, method: &String) -> bool {
        self.config.ignore_methods.contains(method)
    }

    pub fn validate(&self, request: CallRequest, response: CallResult) {
        let clients = self.clients.clone();
        tokio::spawn(async move {
            futures::future::join_all(clients.iter().map(|client| async {
                let expected = client
                    .request(
                        &request.method,
                        request.params.clone(),
                    )
                    .await;

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
                    let endpoint_url = client.endpoints()[0].clone();
                    tracing::error!("Response mismatch for request:\n{request}\nSubway response:\n{actual}\nEndpoint {endpoint_url} response:\n{expected}");
                }
            })).await;
        });
    }
}
