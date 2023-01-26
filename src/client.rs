use futures::TryFutureExt;

use jsonrpsee::types::Params;
pub use jsonrpsee::{
    core::{
        client::{ClientT, Subscription, SubscriptionClientT},
        params::BatchRequestBuilder,
        Error, JsonValue, RpcResult,
    },
    rpc_params,
    ws_client::{WsClient, WsClientBuilder},
};

use crate::config::Config;

pub struct Client {
    ws: WsClient,
}

impl Client {
    pub async fn new(endpoints: impl IntoIterator<Item = impl AsRef<str>>) -> Result<Self, String> {
        let endpoints: Vec<_> = endpoints
            .into_iter()
            .map(|e| e.as_ref().to_string())
            .collect();

        if endpoints.is_empty() {
            return Err("No endpoints provided".into());
        }

        let url = endpoints[0].clone();

        let ws = WsClientBuilder::default()
            .request_timeout(std::time::Duration::from_secs(60))
            .connection_timeout(std::time::Duration::from_secs(30))
            .max_notifs_per_subscription(1024)
            .build(&url)
            .map_err(|e| format!("Unable to connect to endpoint {url}: {e}"))
            .await?;

        Ok(Self { ws })
    }

    pub async fn request(&self, method: &str, params: Params<'_>) -> Result<JsonValue, Error> {
        let params: Vec<JsonValue> = params.parse()?;
        self.ws.request(method, params).await
    }
}

pub async fn create_client(config: &Config) -> anyhow::Result<Client> {
    let endpoints = config.endpoints.clone();

    Client::new(endpoints).await.map_err(anyhow::Error::msg)
}
