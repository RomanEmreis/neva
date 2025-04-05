//! Utilities for extraction params from Request

use serde::de::DeserializeOwned;
use crate::error::{Error, ErrorCode};
use crate::types::Request;

/// A trait that helps the extract typed _params_ from request
pub trait FromRequest: Sized {
    fn from_request(request: Request) -> Result<Self, Error>;
}

impl<T: DeserializeOwned> FromRequest for T {
    fn from_request(req: Request) -> Result<Self, Error> {
        let params = req
            .params
            .ok_or_else(|| Error::new(ErrorCode::InvalidParams, "missing required parameters"))?;

        let params = serde_json::from_value(params)?;
        Ok(params)
    }
}