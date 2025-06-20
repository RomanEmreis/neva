//! Generic identity data structure for requests.

use std::fmt;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use crate::shared::{ArcSlice, ArcStr, MemChr};
use crate::types::ProgressToken;

const SEPARATOR: u8 = b'/';

/// A unique identifier for a request
#[derive(Debug, PartialEq, Eq, Hash)]
pub enum RequestId {
    Number(i64),
    Uuid(uuid::Uuid),
    String(ArcStr),
    Slice(ArcSlice<RequestId>)
}

impl Clone for RequestId {
    #[inline]
    fn clone(&self) -> Self {
        match self {
            RequestId::Number(num) => RequestId::Number(*num),
            RequestId::Uuid(uuid) => RequestId::Uuid(*uuid),
            RequestId::String(str) => RequestId::String(str.clone()),
            RequestId::Slice(slice) => RequestId::Slice(slice.clone())
        }
    }
}

impl Default for RequestId {
    #[inline]
    fn default() -> RequestId {
        Self::String("(no id)".into())
    }
}

impl Display for RequestId {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            RequestId::Number(num) => write!(f, "{}", num),
            RequestId::Uuid(uuid) => write!(f, "{}", uuid),
            RequestId::String(str) => write!(f, "{}", str),
            RequestId::Slice(slice) => write!(f, "{}", slice)
        }
    }
}

impl FromStr for RequestId {
    type Err = String;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(uuid) = uuid::Uuid::parse_str(s) {
            Ok(RequestId::Uuid(uuid))
        } else if let Ok(n) = s.parse::<i64>() {
            Ok(RequestId::Number(n))
        } else {
            Ok(RequestId::String(ArcStr::from(s)))
        }
    }
}

impl From<&RequestId> for ProgressToken {
    #[inline]
    fn from(id: &RequestId) -> ProgressToken {
        match id {
            RequestId::Number(num) => ProgressToken::Number(*num),
            RequestId::Uuid(uuid) => ProgressToken::Uuid(*uuid),
            RequestId::String(str) => ProgressToken::String(str.clone()),
            RequestId::Slice(slice) => ProgressToken::Slice(slice
                .iter()
                .map(Into::into)
                .collect::<Vec<ProgressToken>>()
                .into())
        }
    }
}

impl Serialize for RequestId {
    #[inline]
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            RequestId::Number(n) => serializer.serialize_i64(*n),
            RequestId::Uuid(u) => u.serialize(serializer),
            RequestId::String(s) => s.serialize(serializer),
            RequestId::Slice(p) => p.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for RequestId {
    #[inline]
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(RequestIdVisitor)
    }
}

struct RequestIdVisitor;

impl serde::de::Visitor<'_> for RequestIdVisitor {
    type Value = RequestId;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a number, UUID, string, or slash-separated path")
    }

    #[inline]
    fn visit_i32<E>(self, value: i32) -> Result<Self::Value, E> {
        Ok(RequestId::Number(value as i64))
    }

    #[inline]
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(RequestId::Number(value))
    }

    #[inline]
    fn visit_u32<E>(self, value: u32) -> Result<Self::Value, E> {
        Ok(RequestId::Number(value as i64))
    }

    #[inline]
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(RequestId::Number(value as i64))
    }

    #[inline]
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        if let Ok(uuid) = uuid::Uuid::parse_str(v) {
            Ok(RequestId::Uuid(uuid))
        } else if MemChr::contains(v, SEPARATOR) {
            let parsed = MemChr::split(v, SEPARATOR)
                .map(|s| s.parse::<RequestId>().map_err(E::custom))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(RequestId::Slice(ArcSlice::from(parsed)))
        } else {
            Ok(RequestId::String(ArcStr::from(v)))
        }
    }
}

impl RequestId {
    /// Consumes the current [`RequestId`], concatenates it with another one 
    /// and returns a new [`RequestId::Slice`]
    pub fn concat(self, request_id: RequestId) -> RequestId {
        let slice = [self, request_id];
        Self::Slice(slice.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_converts_numeric_request_id_to_progress_token() {
        let id = RequestId::Number(1);
        let token = ProgressToken::from(&id);
        assert_eq!(token, ProgressToken::Number(1));
    }

    #[test]
    fn it_converts_string_request_id_to_progress_token() {
        let id = RequestId::String("abc".into());
        let token = ProgressToken::from(&id);
        assert_eq!(token, ProgressToken::String("abc".into()));
    }

    #[test]
    fn it_converts_uuid_request_id_to_progress_token() {
        let uuid = uuid::Uuid::new_v4();
        let id = RequestId::Uuid(uuid);
        let token = ProgressToken::from(&id);
        assert_eq!(token, ProgressToken::Uuid(uuid));
    }

    #[test]
    fn it_converts_slice_request_id_to_progress_token() {
        let id = RequestId::Slice([
            RequestId::String("user".into()),
            RequestId::Number(1)
        ].into());
        let token = ProgressToken::from(&id);

        assert_eq!(token, ProgressToken::Slice([
            ProgressToken::String("user".into()),
            ProgressToken::Number(1)
        ].into()));
    }

    #[test]
    fn it_concatenates_request_ids() {
        let id_1 = RequestId::String("id".into());
        let id_2 = RequestId::Number(1);

        let concatenated = id_1.concat(id_2);

        assert_eq!(concatenated.to_string(), "id/1");
    }

    #[test]
    fn it_serializes_and_deserializes_slice_through_str_request_id() {
        let expected_id = RequestId::Slice([
            RequestId::String("user".into()),
            RequestId::Number(1)
        ].into());

        let json = serde_json::to_string(&expected_id).unwrap();
        let new_id: RequestId = serde_json::from_str(&json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_slice_through_value_request_id() {
        let expected_id = RequestId::Slice([
            RequestId::String("user".into()),
            RequestId::Number(1)
        ].into());

        let json = serde_json::to_value(&expected_id).unwrap();
        let new_id: RequestId = serde_json::from_value(json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_numeric_through_str_request_id() {
        let expected_id = RequestId::Number(10);

        let json = serde_json::to_string(&expected_id).unwrap();
        let new_id: RequestId = serde_json::from_str(&json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_numeric_through_value_request_id() {
        let expected_id = RequestId::Number(10);

        let json = serde_json::to_value(&expected_id).unwrap();
        let new_id: RequestId = serde_json::from_value(json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_string_through_str_request_id() {
        let expected_id = RequestId::String("user".into());

        let json = serde_json::to_string(&expected_id).unwrap();
        let new_id: RequestId = serde_json::from_str(&json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_string_through_value_request_id() {
        let expected_id = RequestId::String("user".into());

        let json = serde_json::to_value(&expected_id).unwrap();
        let new_id: RequestId = serde_json::from_value(json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_uuid_through_str_request_id() {
        let expected_id = RequestId::Uuid(uuid::Uuid::new_v4());

        let json = serde_json::to_string(&expected_id).unwrap();
        let new_id: RequestId = serde_json::from_str(&json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_uuid_through_value_request_id() {
        let expected_id = RequestId::Uuid(uuid::Uuid::new_v4());

        let json = serde_json::to_value(&expected_id).unwrap();
        let new_id: RequestId = serde_json::from_value(json).unwrap();

        assert_eq!(expected_id, new_id);
    }
}
