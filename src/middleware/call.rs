use crate::client::Client;
use async_trait::async_trait;
use jsonrpsee::core::{Error, JsonValue};
use std::sync::Arc;

use super::{Middleware, NextFn, Request};

pub struct CallMiddleware {
    client: Arc<Client>,
}

impl CallMiddleware {
    pub fn new(client: &Arc<Client>) -> Self {
        Self {
            client: client.clone(),
        }
    }
}

#[async_trait]
impl Middleware for CallMiddleware {
    async fn call(
        &self,
        request: Request,
        next: NextFn<Request, Result<JsonValue, Error>>,
    ) -> Result<JsonValue, Error> {
        match request {
            Request::Call { method, params } => self.client.request(&method, params.parse()?).await,
            _ => next(request).await,
        }
    }
}
