// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! Middleware that proxies requests at a specified URI to internal
//! RPC method calls.

use hyper::body::Bytes;
use hyper::header::{ACCEPT, CONTENT_TYPE};
use hyper::http::HeaderValue;
use hyper::{Method, Uri};
use jsonrpsee::{
    core::{
        client::Error as RpcError,
        http_helpers::{Body as HttpBody, Request as HttpRequest, Response as HttpResponse},
        BoxError,
    },
    types::{Id, RequestSer},
};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::{Layer, Service};

#[derive(Debug, Clone)]
pub struct ProxyGetRequestMethod {
    pub path: String,
    pub method: String,
}

#[derive(Debug, Clone)]
pub struct ProxyGetRequestLayer {
    methods: Vec<ProxyGetRequestMethod>,
}

impl ProxyGetRequestLayer {
    pub fn new(methods: Vec<ProxyGetRequestMethod>) -> Result<Self, RpcError> {
        for method in &methods {
            if !method.path.starts_with('/') {
                return Err(RpcError::Custom(
                    "ProxyGetRequestLayer path must start with `/`".to_string(),
                ));
            }
        }

        Ok(Self { methods })
    }
}
impl<S> Layer<S> for ProxyGetRequestLayer {
    type Service = ProxyGetRequest<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ProxyGetRequest::new(inner, self.methods.clone()).expect("Path already validated in ProxyGetRequestLayer; qed")
    }
}

#[derive(Debug, Clone)]
pub struct ProxyGetRequest<S> {
    inner: S,
    methods: HashMap<String, String>,
}

impl<S> ProxyGetRequest<S> {
    pub fn new(inner: S, methods: Vec<ProxyGetRequestMethod>) -> Result<Self, RpcError> {
        let mut map = HashMap::with_capacity(methods.len());

        for method in methods {
            if !method.path.starts_with('/') {
                return Err(RpcError::Custom(
                    "ProxyGetRequestLayer path must start with `/`".to_string(),
                ));
            }

            map.insert(method.path, method.method);
        }

        Ok(Self { inner, methods: map })
    }
}

impl<S, B> Service<HttpRequest<B>> for ProxyGetRequest<S>
where
    S: Service<HttpRequest, Response = HttpResponse>,
    S::Response: 'static,
    S::Error: Into<BoxError> + 'static,
    S::Future: Send + 'static,
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Data: Send,
    B::Error: Into<BoxError>,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    #[inline]
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, mut req: HttpRequest<B>) -> Self::Future {
        let method = self.methods.get(req.uri().path());
        let modify = method.is_some() && req.method() == Method::GET;

        // Proxy the request to the appropriate method call.
        let req = if modify {
            // RPC methods are accessed with `POST`.
            *req.method_mut() = Method::POST;
            // Precautionary remove the URI.
            *req.uri_mut() = Uri::from_static("/");

            // Requests must have the following headers:
            req.headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            req.headers_mut()
                .insert(ACCEPT, HeaderValue::from_static("application/json"));
            // Adjust the body to reflect the method call.
            // let bytes = serde_json::to_vec(&RequestSer::borrowed(&Id::Number(0), method.unwrap(), None))
            // .expect("Valid request; qed");

            // let body = HttpBody::from(bytes);
            // Adjust the body to reflect the method call.
            let bytes = serde_json::to_vec(&RequestSer::borrowed(&Id::Number(0), method.unwrap(), None))
                .expect("Valid request; qed");
            let body = HttpBody::from(bytes);

            req.map(|_| body)
        } else {
            req.map(HttpBody::new)
        };

        // Call the inner service and get a future that resolves to the response.
        let fut = self.inner.call(req);

        // Adjust the response if needed.
        let res_fut = async move {
            let res = fut.await.map_err(|err| err.into())?;

            // Nothing to modify: return the response as is.
            if !modify {
                return Ok(res);
            }

            let body = res.into_body();
            let bytes = http_body_util::BodyExt::collect(body).await?.to_bytes();

            #[derive(serde::Deserialize, Debug)]
            struct RpcPayload<'a> {
                #[serde(borrow)]
                result: &'a serde_json::value::RawValue,
            }

            let response = if let Ok(payload) = serde_json::from_slice::<RpcPayload>(&bytes) {
                response::ok_response(payload.result.to_string())
            } else {
                response::internal_error()
            };

            Ok(response)
        };

        Box::pin(res_fut)
    }
}

mod response {
    use jsonrpsee::{
        core::http_helpers::{Body as HttpBody, Response as HttpResponse},
        types::{error::ErrorCode, ErrorObjectOwned, Id, Response, ResponsePayload},
    };

    const JSON: &str = "application/json; charset=utf-8";

    /// Create a response body.
    fn from_template(status: hyper::StatusCode, body: impl Into<HttpBody>, content_type: &'static str) -> HttpResponse {
        HttpResponse::builder()
            .status(status)
            .header("content-type", hyper::header::HeaderValue::from_static(content_type))
            .body(body.into())
            // Parsing `StatusCode` and `HeaderValue` is infalliable but
            // parsing body content is not.
            .expect("Unable to parse response body for type conversion")
    }

    /// Create a valid JSON response.
    pub(crate) fn ok_response(body: impl Into<HttpBody>) -> HttpResponse {
        from_template(hyper::StatusCode::OK, body, JSON)
    }
    /// Create a response for json internal error.
    pub(crate) fn internal_error() -> HttpResponse {
        let err = ResponsePayload::<()>::error(ErrorObjectOwned::from(ErrorCode::InternalError));
        let rp = Response::new(err, Id::Null);
        let error = serde_json::to_string(&rp).expect("built from known-good data; qed");

        from_template(hyper::StatusCode::INTERNAL_SERVER_ERROR, error, JSON)
    }
}
