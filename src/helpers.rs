pub mod errors {
    use jsonrpsee::types::{
        error::{
            CALL_EXECUTION_FAILED_CODE, INTERNAL_ERROR_CODE, INTERNAL_ERROR_MSG,
            INVALID_PARAMS_CODE, INVALID_PARAMS_MSG,
        },
        ErrorObjectOwned,
    };

    pub fn invalid_params<T: ToString>(msg: T) -> ErrorObjectOwned {
        ErrorObjectOwned::owned(
            INVALID_PARAMS_CODE,
            INVALID_PARAMS_MSG,
            Some(msg.to_string()),
        )
    }

    pub fn failed<T: ToString>(msg: T) -> ErrorObjectOwned {
        ErrorObjectOwned::owned(
            CALL_EXECUTION_FAILED_CODE,
            "Call Execution Failed",
            Some(msg.to_string()),
        )
    }

    pub fn internal_error<T: ToString>(msg: T) -> ErrorObjectOwned {
        ErrorObjectOwned::owned(
            INTERNAL_ERROR_CODE,
            INTERNAL_ERROR_MSG,
            Some(msg.to_string()),
        )
    }

    pub fn map_error(err: jsonrpsee::core::Error) -> ErrorObjectOwned {
        use jsonrpsee::core::Error::*;
        match err {
            Call(e) => e,
            x => internal_error(x),
        }
    }
}

pub mod telemetry {
    use opentelemetry::{
        global::{self, BoxedSpan},
        trace::{TraceContextExt, Tracer as _},
        Context,
    };
    use std::borrow::Cow;

    #[derive(Clone, Copy, Debug)]
    pub struct Tracer(&'static str);

    impl Tracer {
        pub const fn new(name: &'static str) -> Self {
            Self(name)
        }

        pub fn span(&self, span_name: impl Into<Cow<'static, str>>) -> BoxedSpan {
            global::tracer(self.0).start(span_name)
        }

        pub fn context(&self, span_name: impl Into<Cow<'static, str>>) -> Context {
            let span = self.span(span_name);
            Context::current_with_span(span)
        }
    }
}
