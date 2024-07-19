use crate::middlewares::CallResult;
use jsonrpsee::{
    core::{
        client::{ClientT, Error},
        JsonValue,
    },
    http_client::HttpClient as RpcClient,
    types::{error::INTERNAL_ERROR_CODE, ErrorObject},
};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Very simple struct to have a set of JsonRpsee HTTP clients and send requests to them
pub struct HttpClient {
    clients: Vec<RpcClient>,
    last_sent: AtomicUsize,
}

impl HttpClient {
    pub fn new(endpoints: &[String]) -> Result<Self, Error> {
        let clients = endpoints
            .iter()
            .map(|url| RpcClient::builder().build(url))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            clients,
            last_sent: AtomicUsize::new(0),
        })
    }

    /// Sends a request to one of the clients
    ///
    /// The client is selected in a round-robin fashion as fair as possible
    pub async fn request(&self, method: &str, params: Vec<JsonValue>) -> CallResult {
        let client_id = self.last_sent.fetch_add(1, Ordering::Relaxed) % self.clients.len();

        self.clients[client_id]
            .request(method, params)
            .await
            .map_err(|e| match e {
                jsonrpsee::core::client::Error::Call(e) => e,
                e => ErrorObject::owned(INTERNAL_ERROR_CODE, e.to_string(), None::<String>),
            })
    }
}
