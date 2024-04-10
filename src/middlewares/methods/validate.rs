use async_trait::async_trait;
use std::sync::Arc;
use std::{fs::File, io::Read};

use crate::utils::errors;
use crate::{
    extensions::client::Client,
    middlewares::{CallRequest, CallResult, Middleware, MiddlewareBuilder, NextFn, RpcMethod},
    utils::{TypeRegistry, TypeRegistryRef},
};

pub struct ValidateMiddleware {
    client: Arc<Client>,
    ignore_methods: Vec<String>,
}

impl ValidateMiddleware {
    pub fn new(client: Arc<Client>, ignore_methods: Vec<String>) -> Self {
        Self { client, ignore_methods }
    }
}

#[async_trait]
impl MiddlewareBuilder<RpcMethod, CallRequest, CallResult> for ValidateMiddleware {
    async fn build(
        _method: &RpcMethod,
        extensions: &TypeRegistryRef,
    ) -> Option<Box<dyn Middleware<CallRequest, CallResult>>> {
        // read ignored methods from file
        let mut ignore_methods = vec![];
        if let Ok(mut file) = File::open(".validateignore") {
            if let Err(err) = file.read_to_end(&mut ignore_methods) {
                tracing::error!("Read .validateignore failed: {err:?}");
            }
        }
        let ignore_methods = String::from_utf8(ignore_methods)
            .unwrap_or_default()
            .split('\n')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.starts_with('#') && !x.starts_with("//")) // filter comments
            .collect();

        let client = extensions
            .read()
            .await
            .get::<Client>()
            .expect("Client extension not found");
        Some(Box::new(ValidateMiddleware::new(client, ignore_methods)))
    }
}

#[async_trait]
impl Middleware<CallRequest, CallResult> for ValidateMiddleware {
    async fn call(
        &self,
        request: CallRequest,
        context: TypeRegistry,
        next: NextFn<CallRequest, CallResult>,
    ) -> CallResult {
        let client = self.client.clone();
        let result = next(request.clone(), context).await;
        let actual = result.clone();

        if self.ignore_methods.contains(&request.method) {
            return result;
        }

        if let Err(err) = tokio::spawn(async move {
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

                if actual != expected {
                    let request = serde_json::to_string_pretty(&request).unwrap_or_default();
                    let actual = match &actual {
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
        }).await {
            tracing::error!("Validate task failed: {err:?}");
        }

        result
    }
}
