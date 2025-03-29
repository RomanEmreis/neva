//! Represents a request from MCP client

use std::fmt;
use serde::{Serialize, Deserialize, de::DeserializeOwned};
use crate::types::CallToolRequestParams;

/// A unique identifier for a request
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Number(i64),
}

impl Default for RequestId {
    #[inline]
    fn default() -> RequestId {
        Self::String("(no id)".into())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<serde_json::Value>,
    pub id: Option<RequestId>,
}

impl fmt::Display for RequestId {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestId::String(str) => write!(f, "{}", str),
            RequestId::Number(num) => write!(f, "{}", num),
        }
    }
}

impl TryFrom<Request> for () {
    type Error = String;
    
    #[inline]
    fn try_from(_: Request) -> Result<Self, Self::Error> {
        Ok(())
    }
}

macro_rules! impl_from_request {
    ($($T: ident),*) => {
        impl<$($T: DeserializeOwned),+> TryFrom<Request> for ($($T,)+) {
            type Error = String;
            
            #[inline]
            fn try_from(req: Request) -> Result<Self, Self::Error> {
                let params = match req.params {
                    Some(params) => serde_json::from_value::<CallToolRequestParams>(params).map_err(|err| err.to_string()),
                    None => Err("unable to read params".into())
                };
                let args = params?.args.unwrap();
                let mut iter = args.iter();
                let tuple = (
                    $(
                    $T::deserialize(iter.next().unwrap().1.clone()).map_err(|err| err.to_string())?,
                    )*    
                );
                Ok(tuple)
            }
        }
    }
}

impl_from_request! { T1 }
impl_from_request! { T1, T2 }
impl_from_request! { T1, T2, T3 }
impl_from_request! { T1, T2, T3, T4 }
impl_from_request! { T1, T2, T3, T4, T5 }

#[cfg(test)]
mod tests {

}