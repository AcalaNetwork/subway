use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use garde::Validate;
use jsonrpsee::core::{
    client::{Error, Subscription},
    JsonValue,
};
use rand::{seq::SliceRandom, thread_rng};
use serde::Deserialize;

use super::ExtensionRegistry;
use crate::{extensions::Extension, middlewares::CallResult, utils::errors};

mod http;
#[cfg(test)]
pub mod mock;
#[cfg(test)]
mod tests;
#[allow(dead_code)]
mod ws;

pub struct Client {
    endpoints: Vec<String>,
    http_client: Option<http::HttpClient>,
    ws_client: Option<ws::Client>,
}

#[derive(Deserialize, Validate, Debug)]
#[garde(allow_unvalidated)]
pub struct ClientConfig {
    #[garde(inner(custom(validate_endpoint)))]
    pub endpoints: Vec<String>,
    #[serde(default = "ws::bool_true")]
    pub shuffle_endpoints: bool,
}

fn validate_endpoint(endpoint: &str, _context: &()) -> garde::Result {
    endpoint
        .parse::<jsonrpsee::client_transport::ws::Uri>()
        .map_err(|_| garde::Error::new(format!("Invalid endpoint format: {}", endpoint)))?;

    Ok(())
}

impl ClientConfig {
    pub async fn all_endpoints_can_be_connected(&self) -> bool {
        let (ws_clients, _) = Client::get_urls(&self.endpoints);

        if ws_clients.is_empty() {
            return true;
        }

        ws::ClientConfig {
            endpoints: ws_clients,
            shuffle_endpoints: self.shuffle_endpoints,
        }
        .all_endpoints_can_be_connected()
        .await
    }
}

#[async_trait]
impl Extension for Client {
    type Config = ClientConfig;

    async fn from_config(config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        if config.shuffle_endpoints {
            let mut endpoints = config.endpoints.clone();
            endpoints.shuffle(&mut thread_rng());
            Ok(Self::new(endpoints, None, None, None)?)
        } else {
            Ok(Self::new(config.endpoints.clone(), None, None, None)?)
        }
    }
}

impl Client {
    pub fn new(
        endpoints: impl IntoIterator<Item = impl AsRef<str>>,
        request_timeout: Option<Duration>,
        connection_timeout: Option<Duration>,
        retries: Option<u32>,
    ) -> Result<Self, anyhow::Error> {
        let endpoints = endpoints
            .into_iter()
            .map(|e| e.as_ref().to_string())
            .collect::<Vec<_>>();
        let (ws_endpoints, http_endpoints) = Self::get_urls(&endpoints);
        if ws_endpoints.is_empty() && http_endpoints.is_empty() {
            return Err(anyhow!("No endpoints provided"));
        }

        Ok(Self {
            endpoints,
            http_client: if http_endpoints.is_empty() {
                None
            } else {
                Some(http::HttpClient::new(&http_endpoints)?)
            },
            ws_client: if ws_endpoints.is_empty() {
                None
            } else {
                Some(ws::Client::new(
                    &ws_endpoints,
                    request_timeout,
                    connection_timeout,
                    retries,
                )?)
            },
        })
    }

    pub fn get_urls(endpoints: impl IntoIterator<Item = impl AsRef<str>>) -> (Vec<String>, Vec<String>) {
        let endpoints = endpoints
            .into_iter()
            .map(|e| e.as_ref().to_string())
            .collect::<Vec<_>>();
        (
            endpoints
                .iter()
                .filter(|e| e.starts_with("ws://") || e.starts_with("wss://"))
                .map(|c| c.to_string())
                .collect::<Vec<_>>(),
            endpoints
                .into_iter()
                .filter(|e| e.starts_with("http://") || e.starts_with("https://"))
                .collect::<Vec<_>>(),
        )
    }

    pub fn with_endpoints(endpoints: impl IntoIterator<Item = impl AsRef<str>>) -> Result<Self, anyhow::Error> {
        Self::new(endpoints, None, None, None)
    }

    pub fn endpoints(&self) -> &Vec<String> {
        &self.endpoints
    }

    pub async fn request(&self, method: &str, params: Vec<JsonValue>) -> CallResult {
        if let Some(http_client) = &self.http_client {
            http_client.request(method, params).await
        } else if let Some(ws_client) = &self.ws_client {
            ws_client.request(method, params).await
        } else {
            Err(errors::internal_error("No upstream client"))
        }
    }

    pub async fn subscribe(
        &self,
        subscribe: &str,
        params: Vec<JsonValue>,
        unsubscribe: &str,
    ) -> Result<Subscription<JsonValue>, Error> {
        if let Some(ws_client) = &self.ws_client {
            ws_client.subscribe(subscribe, params, unsubscribe).await
        } else {
            Err(Error::Call(errors::internal_error("No websocket connection")))
        }
    }

    pub async fn rotate_endpoint(&self) {
        if let Some(ws_client) = &self.ws_client {
            ws_client.rotate_endpoint().await;
        }
    }

    /// Returns a future that resolves when the endpoint is rotated.
    pub async fn on_rotation(&self) {
        if let Some(ws_client) = &self.ws_client {
            ws_client.on_rotation().await;
        }
    }
}
