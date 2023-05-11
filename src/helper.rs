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
