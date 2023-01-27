use futures::TryFutureExt;

use jsonrpsee::{
    core::{
        client::{ClientT, Subscription, SubscriptionClientT},
        Error, JsonValue,
    },
    types::{error::CallError, Params},
    ws_client::WsClientBuilder,
};

use crate::config::Config;

pub struct Client {
    sender: tokio::sync::mpsc::Sender<Message>,
}

#[derive(Debug)]
enum Message {
    Request {
        method: String,
        params: Vec<JsonValue>,
        response: tokio::sync::oneshot::Sender<Result<JsonValue, Error>>,
    },
    Subscribe {
        subscribe: String,
        params: Vec<JsonValue>,
        unsubscribe: String,
        response: tokio::sync::oneshot::Sender<Result<Subscription<JsonValue>, Error>>,
    },
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

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(100);

        tokio::spawn(async move {
            let url = endpoints[0].clone();
            let ws = WsClientBuilder::default()
                .request_timeout(std::time::Duration::from_secs(60))
                .connection_timeout(std::time::Duration::from_secs(30))
                .max_notifs_per_subscription(1024)
                .build(&url)
                .map_err(|e| format!("Unable to connect to endpoint {url}: {e}"))
                .await
                .unwrap();

            loop {
                let message = rx.recv().await.unwrap();

                match message {
                    Message::Request {
                        method,
                        params,
                        response,
                    } => {
                        let res = ws.request(&method, params).await;
                        if response.send(res).is_err() {
                            log::warn!("Unable to send response");
                        }
                    }
                    Message::Subscribe {
                        subscribe,
                        params,
                        unsubscribe,
                        response,
                    } => {
                        let res = ws.subscribe(&subscribe, params, &unsubscribe).await;
                        if response.send(res).is_err() {
                            log::warn!("Unable to send response");
                        }
                    }
                }
            }
        });

        Ok(Self { sender: tx })
    }

    pub async fn request(&self, method: &str, params: Params<'_>) -> Result<JsonValue, Error> {
        let params: Vec<JsonValue> = params.parse()?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender
            .send(Message::Request {
                method: method.into(),
                params,
                response: tx,
            })
            .await
            .map_err(|e| CallError::Failed(e.into()))?;

        rx.await.map_err(|e| CallError::Failed(e.into()))?
    }

    pub async fn subscribe(
        &self,
        subscribe: &str,
        params: Params<'_>,
        unsubscribe: &str,
    ) -> Result<Subscription<JsonValue>, Error> {
        let params: Vec<JsonValue> = params.parse()?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender
            .send(Message::Subscribe {
                subscribe: subscribe.into(),
                params,
                unsubscribe: unsubscribe.into(),
                response: tx,
            })
            .await
            .map_err(|e| CallError::Failed(e.into()))?;

        rx.await.map_err(|e| CallError::Failed(e.into()))?
    }
}

pub async fn create_client(config: &Config) -> anyhow::Result<Client> {
    let endpoints = config.endpoints.clone();

    Client::new(endpoints).await.map_err(anyhow::Error::msg)
}
