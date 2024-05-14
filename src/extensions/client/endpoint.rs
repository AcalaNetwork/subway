use super::health::{self, Event, Health};
use crate::extensions::client::{get_backoff_time, HealthCheckConfig};
use jsonrpsee::{
    async_client::Client,
    core::client::{ClientT, Subscription, SubscriptionClientT},
    core::JsonValue,
    ws_client::WsClientBuilder,
};
use std::{
    fmt::{Debug, Formatter},
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};

enum Message {
    Request {
        method: String,
        params: Vec<JsonValue>,
        response: tokio::sync::oneshot::Sender<Result<JsonValue, jsonrpsee::core::Error>>,
        timeout: Duration,
    },
    Subscribe {
        subscribe: String,
        params: Vec<JsonValue>,
        unsubscribe: String,
        response: tokio::sync::oneshot::Sender<Result<Subscription<JsonValue>, jsonrpsee::core::Error>>,
        timeout: Duration,
    },
    Reconnect,
}

impl Debug for Message {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Message::Request {
                method,
                params,
                response: _,
                timeout,
            } => write!(f, "Request({method}, {params:?}, _, {timeout:?})"),
            Message::Subscribe {
                subscribe,
                params,
                unsubscribe,
                response: _,
                timeout,
            } => write!(f, "Subscribe({subscribe}, {params:?}, {unsubscribe}, _, {timeout:?})"),
            Message::Reconnect => write!(f, "Reconnect"),
        }
    }
}

enum State {
    Initial,
    OnError(health::Event),
    Connect(Option<Message>),
    HandleMessage(Arc<Client>, Message),
    WaitForMessage(Arc<Client>),
}

impl Debug for State {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            State::Initial => write!(f, "Initial"),
            State::OnError(e) => write!(f, "OnError({e:?})"),
            State::Connect(m) => write!(f, "Connect({m:?})"),
            State::HandleMessage(_c, m) => write!(f, "HandleMessage(_, {m:?})"),
            State::WaitForMessage(_c) => write!(f, "WaitForMessage(_)"),
        }
    }
}

pub struct Endpoint {
    url: String,
    health: Arc<Health>,
    message_tx: tokio::sync::mpsc::Sender<Message>,
    background_tasks: Vec<tokio::task::JoinHandle<()>>,
    connect_counter: Arc<AtomicU32>,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.background_tasks.drain(..).for_each(|handle| handle.abort());
    }
}

impl Endpoint {
    pub fn new(
        url: String,
        request_timeout: Duration,
        connection_timeout: Duration,
        health_config: Option<HealthCheckConfig>,
    ) -> Self {
        tracing::info!("New endpoint: {url}");

        let health = Arc::new(Health::new(url.clone()));
        let connect_counter = Arc::new(AtomicU32::new(0));
        let (message_tx, message_rx) = tokio::sync::mpsc::channel::<Message>(4096);

        let mut endpoint = Self {
            url: url.clone(),
            health: health.clone(),
            message_tx,
            background_tasks: vec![],
            connect_counter: connect_counter.clone(),
        };

        endpoint.start_background_task(
            url,
            request_timeout,
            connection_timeout,
            connect_counter,
            message_rx,
            health,
        );
        if let Some(config) = health_config {
            endpoint.start_health_monitor_task(config);
        }

        endpoint
    }

    fn start_background_task(
        &mut self,
        url: String,
        request_timeout: Duration,
        connection_timeout: Duration,
        connect_counter: Arc<AtomicU32>,
        mut message_rx: tokio::sync::mpsc::Receiver<Message>,
        health: Arc<Health>,
    ) {
        let handler = tokio::spawn(async move {
            let connect_backoff_counter = Arc::new(AtomicU32::new(0));

            let mut state = State::Initial;

            loop {
                tracing::trace!("{url} {state:?}");

                let new_state = match state {
                    State::Initial => {
                        connect_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                        // wait for messages before connecting
                        let msg = match message_rx.recv().await {
                            Some(Message::Reconnect) => None,
                            Some(msg @ Message::Request { .. } | msg @ Message::Subscribe { .. }) => Some(msg),
                            None => {
                                let url = url.clone();
                                // channel is closed? exit
                                tracing::debug!("Endpoint {url} channel closed");
                                return;
                            }
                        };
                        State::Connect(msg)
                    }
                    State::OnError(evt) => {
                        health.update(evt);
                        tokio::time::sleep(get_backoff_time(&connect_backoff_counter)).await;
                        State::Initial
                    }
                    State::Connect(msg) => {
                        // TODO: make the params configurable
                        let client = WsClientBuilder::default()
                            .request_timeout(request_timeout)
                            .connection_timeout(connection_timeout)
                            .max_buffer_capacity_per_subscription(2048)
                            .max_concurrent_requests(2048)
                            .max_response_size(20 * 1024 * 1024)
                            .build(url.clone())
                            .await;

                        match client {
                            Ok(client) => {
                                connect_backoff_counter.store(0, std::sync::atomic::Ordering::Relaxed);
                                health.update(Event::ConnectionSuccessful);
                                if let Some(msg) = msg {
                                    State::HandleMessage(Arc::new(client), msg)
                                } else {
                                    State::WaitForMessage(Arc::new(client))
                                }
                            }
                            Err(err) => {
                                tracing::debug!("Endpoint {url} connection error: {err}");
                                State::OnError(health::Event::ConnectionClosed)
                            }
                        }
                    }
                    State::HandleMessage(client, msg) => match msg {
                        Message::Request {
                            method,
                            params,
                            response,
                            timeout,
                        } => {
                            // don't block on making the request
                            let url = url.clone();
                            let health = health.clone();
                            let client2 = client.clone();
                            tokio::spawn(async move {
                                let resp = match tokio::time::timeout(
                                    timeout,
                                    client2.request::<serde_json::Value, Vec<serde_json::Value>>(&method, params),
                                )
                                .await
                                {
                                    Ok(resp) => resp,
                                    Err(_) => {
                                        tracing::warn!("Endpoint {url} request timeout: {method} timeout: {timeout:?}");
                                        health.update(Event::RequestTimeout);
                                        Err(jsonrpsee::core::Error::RequestTimeout)
                                    }
                                };
                                if let Err(err) = &resp {
                                    health.on_error(err);
                                }

                                if response.send(resp).is_err() {
                                    tracing::error!("Unable to send response to message channel");
                                }
                            });

                            State::WaitForMessage(client)
                        }
                        Message::Subscribe {
                            subscribe,
                            params,
                            unsubscribe,
                            response,
                            timeout,
                        } => {
                            // don't block on making the request
                            let url = url.clone();
                            let health = health.clone();
                            let client2 = client.clone();
                            tokio::spawn(async move {
                                let resp = match tokio::time::timeout(
                                    timeout,
                                    client2.subscribe::<serde_json::Value, Vec<serde_json::Value>>(
                                        &subscribe,
                                        params,
                                        &unsubscribe,
                                    ),
                                )
                                .await
                                {
                                    Ok(resp) => resp,
                                    Err(_) => {
                                        tracing::warn!("Endpoint {url} subscription timeout: {subscribe}");
                                        health.update(Event::RequestTimeout);
                                        Err(jsonrpsee::core::Error::RequestTimeout)
                                    }
                                };
                                if let Err(err) = &resp {
                                    health.on_error(err);
                                }

                                if response.send(resp).is_err() {
                                    tracing::error!("Unable to send response to message channel");
                                }
                            });

                            State::WaitForMessage(client)
                        }
                        Message::Reconnect => State::Initial,
                    },
                    State::WaitForMessage(client) => {
                        tokio::select! {
                            msg = message_rx.recv() => {
                                match msg {
                                    Some(msg) => State::HandleMessage(client, msg),
                                    None => {
                                        // channel is closed? exit
                                        tracing::debug!("Endpoint {url} channel closed");
                                        return
                                    }
                                }

                            },
                            () = client.on_disconnect() => {
                                tracing::debug!("Endpoint {url} disconnected");
                                State::OnError(health::Event::ConnectionClosed)
                            }
                        }
                    }
                };

                state = new_state;
            }
        });

        self.background_tasks.push(handler);
    }

    fn start_health_monitor_task(&mut self, config: HealthCheckConfig) {
        let message_tx = self.message_tx.clone();
        let health = self.health.clone();
        let url = self.url.clone();

        let handler = tokio::spawn(async move {
            let health_response = config.response.clone();
            let interval = Duration::from_secs(config.interval_sec);
            let healthy_response_time = Duration::from_millis(config.healthy_response_time_ms);
            let max_response_time: Duration = Duration::from_millis(config.healthy_response_time_ms * 2);

            loop {
                // Wait for the next interval
                tokio::time::sleep(interval).await;

                let request_start = std::time::Instant::now();

                let (response_tx, response_rx) = tokio::sync::oneshot::channel();
                let res = message_tx
                    .send(Message::Request {
                        method: config.health_method.clone(),
                        params: vec![],
                        response: response_tx,
                        timeout: max_response_time,
                    })
                    .await;

                if let Err(err) = res {
                    tracing::error!("{url} Unexpected error in message channel: {err}");
                }

                let res = match response_rx.await {
                    Ok(resp) => resp,
                    Err(err) => {
                        tracing::error!("{url} Unexpected error in response channel: {err}");
                        Err(jsonrpsee::core::Error::Custom("Internal server error".into()))
                    }
                };

                match res {
                    Ok(response) => {
                        let duration = request_start.elapsed();

                        // Check response
                        if let Some(ref health_response) = health_response {
                            if !health_response.validate(&response) {
                                health.update(Event::Unhealthy);
                                continue;
                            }
                        }

                        // Check response time
                        if duration > healthy_response_time {
                            tracing::warn!("{url} response time is too long: {duration:?}");
                            health.update(Event::SlowResponse);
                            continue;
                        }

                        health.update(Event::ResponseOk);
                    }
                    Err(err) => {
                        health.on_error(&err);
                    }
                }
            }
        });

        self.background_tasks.push(handler);
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn health(&self) -> &Health {
        self.health.as_ref()
    }

    pub fn connect_counter(&self) -> u32 {
        self.connect_counter.load(Ordering::Relaxed)
    }

    pub async fn request(
        &self,
        method: &str,
        params: Vec<serde_json::Value>,
        timeout: Duration,
    ) -> Result<serde_json::Value, jsonrpsee::core::Error> {
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let res = self
            .message_tx
            .send(Message::Request {
                method: method.into(),
                params,
                response: response_tx,
                timeout,
            })
            .await;

        if let Err(err) = res {
            tracing::error!("Unexpected error in message channel: {err}");
        }

        match response_rx.await {
            Ok(resp) => resp,
            Err(err) => {
                tracing::error!("Unexpected error in response channel: {err}");
                Err(jsonrpsee::core::Error::Custom("Internal server error".into()))
            }
        }
    }

    pub async fn subscribe(
        &self,
        subscribe_method: &str,
        params: Vec<serde_json::Value>,
        unsubscribe_method: &str,
        timeout: Duration,
    ) -> Result<Subscription<serde_json::Value>, jsonrpsee::core::Error> {
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let res = self
            .message_tx
            .send(Message::Subscribe {
                subscribe: subscribe_method.into(),
                params,
                unsubscribe: unsubscribe_method.into(),
                response: response_tx,
                timeout,
            })
            .await;

        if let Err(err) = res {
            tracing::error!("Unexpected error in message channel: {err}");
        }

        match response_rx.await {
            Ok(resp) => resp,
            Err(err) => {
                tracing::error!("Unexpected error in response channel: {err}");
                Err(jsonrpsee::core::Error::Custom("Internal server error".into()))
            }
        }
    }

    pub async fn reconnect(&self) {
        let res = self.message_tx.send(Message::Reconnect).await;
        if let Err(err) = res {
            tracing::error!("Unexpected error in message channel: {err}");
        }
    }
}
