//! Serializable [`Arc<[T]>`]

use std::fmt;
use std::fmt::Display;
use std::ops::Deref;
use std::sync::Arc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArcSlice<T>(Arc<[T]>);

impl<T: Display> Display for ArcSlice<T> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, seg) in self.iter().enumerate() {
            if i > 0 {
                write!(f, "/{seg}")?;
            } else { 
                write!(f, "{seg}")?;
            }
        }
        Ok(())
    }
}

impl<T> Deref for ArcSlice<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> From<Vec<T>> for ArcSlice<T> {
    #[inline]
    fn from(vec: Vec<T>) -> Self {
        Self(Arc::from(vec))
    }
}

impl<const N: usize, T> From<[T; N]> for ArcSlice<T> {
    #[inline]
    fn from(value: [T; N]) -> Self {
        Self(Arc::from(value))
    }
}

impl<T> From<Arc<[T]>> for ArcSlice<T>  {
    #[inline]
    fn from(arc: Arc<[T]>) -> Self {
        Self(arc)   
    }
}

impl<T> Serialize for ArcSlice<T>
where 
    T: Display
{
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de, T> Deserialize<'de> for ArcSlice<T>
where
    T: std::str::FromStr,
    T::Err: Display,
{
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>
    {
        let s: &str = Deserialize::deserialize(deserializer)?;
        let parsed = s
            .split('/')
            .map(|part| part.parse().map_err(serde::de::Error::custom))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ArcSlice(Arc::from(parsed)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use uuid::Uuid;
    use super::*;
    use crate::{shared::ArcStr, types::RequestId};

    #[test]
    fn it_tests_display() {
        let slice = ArcSlice(Arc::from([
            RequestId::String(ArcStr::from("user")),
            RequestId::Number(42),
        ]));
        assert_eq!(slice.to_string(), "user/42");
    }

    #[test]
    fn it_serializes_arc_slice_request_id() {
        let slice = ArcSlice(Arc::from([
            RequestId::Uuid(Uuid::parse_str("b9d3c680-bb27-4d7d-9e76-111111111111").unwrap()),
            RequestId::Number(1),
            RequestId::String(ArcStr::from("abc")),
        ]));

        let json = serde_json::to_string(&slice).unwrap();
        assert_eq!(
            json,
            "\"b9d3c680-bb27-4d7d-9e76-111111111111/1/abc\""
        );
    }

    #[test]
    fn it_deserializes_arc_slice_request_id() {
        let json = "\"b9d3c680-bb27-4d7d-9e76-111111111111/1/abc\"";
        let slice: ArcSlice<RequestId> = serde_json::from_str(json).unwrap();

        assert_eq!(
            slice.0.as_ref(),
            &[
                RequestId::Uuid(Uuid::parse_str("b9d3c680-bb27-4d7d-9e76-111111111111").unwrap()),
                RequestId::Number(1),
                RequestId::String(ArcStr::from("abc")),
            ]
        );
    }

    #[test]
    fn it_roundtrips_arc_slice() {
        let original = ArcSlice(Arc::from([
            RequestId::String(ArcStr::from("project")),
            RequestId::Number(123),
        ]));

        let json = serde_json::to_string(&original).unwrap();
        let decoded: ArcSlice<RequestId> = serde_json::from_str(&json).unwrap();

        assert_eq!(original, decoded);
    }
}