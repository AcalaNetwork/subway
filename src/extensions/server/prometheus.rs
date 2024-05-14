use futures::{future::BoxFuture, FutureExt};
use jsonrpsee::server::middleware::rpc::RpcServiceT;
use jsonrpsee::types::Request;
use jsonrpsee::MethodResponse;
use substrate_prometheus_endpoint::{CounterVec, HistogramVec, U64};

use std::fmt::Display;

#[derive(Clone, Copy)]
pub enum Protocol {
    Ws,
    Http,
}

impl Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Self::Ws => "ws",
            Self::Http => "http",
        };
        write!(f, "{}", str)
    }
}

#[derive(Clone)]
pub struct PrometheusService<S> {
    inner: S,
    protocol: Protocol,
    call_times: HistogramVec,
    calls_started: CounterVec<U64>,
    calls_finished: CounterVec<U64>,
}

impl<S> PrometheusService<S> {
    pub fn new(
        inner: S,
        protocol: Protocol,
        call_times: &HistogramVec,
        calls_started: &CounterVec<U64>,
        calls_finished: &CounterVec<U64>,
    ) -> Self {
        Self {
            inner,
            protocol,
            calls_started: calls_started.clone(),
            calls_finished: calls_finished.clone(),
            call_times: call_times.clone(),
        }
    }
}

impl<'a, S> RpcServiceT<'a> for PrometheusService<S>
where
    S: RpcServiceT<'a> + Send + Sync + Clone + 'static,
{
    type Future = BoxFuture<'a, MethodResponse>;

    fn call(&self, req: Request<'a>) -> Self::Future {
        let protocol = self.protocol.to_string();
        let method = req.method.to_string();

        let histogram = self.call_times.with_label_values(&[&protocol, &method]);
        let started = self.calls_started.with_label_values(&[&protocol, &method]);
        let finished = self.calls_finished.clone();

        let service = self.inner.clone();
        async move {
            started.inc();

            let timer = histogram.start_timer();
            let res = service.call(req).await;
            timer.stop_and_record();
            finished
                .with_label_values(&[&protocol, &method, &res.is_error().to_string()])
                .inc();

            res
        }
        .boxed()
    }
}
