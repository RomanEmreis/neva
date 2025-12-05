//! Types and utilities for the "either" pattern

use serde::{Serialize, Serializer};
#[cfg(feature = "server")]
use crate::types::{IntoResponse, RequestId, Response};

/// Represents a value of one of two types
#[derive(Debug)]
pub enum Either<L, R> {
    /// Left value
    Left(L),
    
    /// Right value
    Right(R),
}

impl<L, R> Serialize for Either<L, R> 
where
    L: Serialize,
    R: Serialize
{
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self { 
            Either::Left(l) => l.serialize(serializer),
            Either::Right(r) => r.serialize(serializer)
        }
    }   
}

#[cfg(feature = "server")]
impl<L, R> IntoResponse for Either<L, R>
where
    L: IntoResponse,
    R: IntoResponse
{
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match self { 
            Either::Left(l) => l.into_response(req_id),
            Either::Right(r) => r.into_response(req_id)
        }
    }
}