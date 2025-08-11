//! Utilities for tracking operation's progress

use std::fmt;
use std::fmt::Display;
use std::str::FromStr;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use crate::shared::{ArcSlice, ArcStr, MemChr};
use crate::types::notification::ProgressNotification;

const SEPARATOR: u8 = b'/';

/// Represents a progress token, which can be either a string or an integer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProgressToken {
    Number(i64),
    Uuid(uuid::Uuid),
    String(ArcStr),
    Slice(ArcSlice<ProgressToken>)
}

impl Display for ProgressToken {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProgressToken::Number(n) => write!(f, "{n}"),
            ProgressToken::Uuid(u) => write!(f, "{u}"),
            ProgressToken::String(s) => write!(f, "{s}"),
            ProgressToken::Slice(s) => write!(f, "{s}")
        }
    }
}

impl FromStr for ProgressToken {
    type Err = String;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(uuid) = uuid::Uuid::parse_str(s) {
            Ok(ProgressToken::Uuid(uuid))
        } else if let Ok(n) = s.parse::<i64>() {
            Ok(ProgressToken::Number(n))
        } else {
            Ok(ProgressToken::String(ArcStr::from(s)))
        }
    }
}

impl Serialize for ProgressToken {
    #[inline]
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ProgressToken::Number(n) => serializer.serialize_i64(*n),
            ProgressToken::Uuid(u) => u.serialize(serializer),
            ProgressToken::String(s) => s.serialize(serializer),
            ProgressToken::Slice(p) => p.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ProgressToken {
    #[inline]
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ProgressTokenVisitor)
    }
}

struct ProgressTokenVisitor;

impl serde::de::Visitor<'_> for ProgressTokenVisitor {
    type Value = ProgressToken;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a number, UUID, string, or slash-separated path")
    }

    #[inline]
    fn visit_i32<E>(self, value: i32) -> Result<Self::Value, E> {
        Ok(ProgressToken::Number(value as i64))
    }

    #[inline]
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(ProgressToken::Number(value))
    }

    #[inline]
    fn visit_u32<E>(self, value: u32) -> Result<Self::Value, E> {
        Ok(ProgressToken::Number(value as i64))
    }

    #[inline]
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(ProgressToken::Number(value as i64))
    }

    #[inline]
    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        if let Ok(uuid) = uuid::Uuid::parse_str(v) {
            Ok(ProgressToken::Uuid(uuid))
        } else if MemChr::contains(v, SEPARATOR) {
            let parsed = MemChr::split(v, SEPARATOR)
                .map(|s| s.parse::<ProgressToken>().map_err(E::custom))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ProgressToken::Slice(ArcSlice::from(parsed)))
        } else {
            Ok(ProgressToken::String(ArcStr::from(v)))
        }
    }
}

impl ProgressToken {
    /// Creates a [`ProgressNotification`]
    pub fn notify(&self, progress: f64, total: Option<f64>) -> ProgressNotification {
        ProgressNotification {
            progress_token: self.clone(),
            progress,
            total
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_serializes_and_deserializes_slice_through_str_request_id() {
        let expected_id = ProgressToken::Slice([
            ProgressToken::String("user".into()),
            ProgressToken::Number(1)
        ].into());

        let json = serde_json::to_string(&expected_id).unwrap();
        let new_id: ProgressToken = serde_json::from_str(&json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_slice_through_value_request_id() {
        let expected_id = ProgressToken::Slice([
            ProgressToken::String("user".into()),
            ProgressToken::Number(1)
        ].into());

        let json = serde_json::to_value(&expected_id).unwrap();
        let new_id: ProgressToken = serde_json::from_value(json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_numeric_through_str_request_id() {
        let expected_id = ProgressToken::Number(10);

        let json = serde_json::to_string(&expected_id).unwrap();
        let new_id: ProgressToken = serde_json::from_str(&json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_numeric_through_value_request_id() {
        let expected_id = ProgressToken::Number(10);

        let json = serde_json::to_value(&expected_id).unwrap();
        let new_id: ProgressToken = serde_json::from_value(json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_string_through_str_request_id() {
        let expected_id = ProgressToken::String("user".into());

        let json = serde_json::to_string(&expected_id).unwrap();
        let new_id: ProgressToken = serde_json::from_str(&json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_string_through_value_request_id() {
        let expected_id = ProgressToken::String("user".into());

        let json = serde_json::to_value(&expected_id).unwrap();
        let new_id: ProgressToken = serde_json::from_value(json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_uuid_through_str_request_id() {
        let expected_id = ProgressToken::Uuid(uuid::Uuid::new_v4());

        let json = serde_json::to_string(&expected_id).unwrap();
        let new_id: ProgressToken = serde_json::from_str(&json).unwrap();

        assert_eq!(expected_id, new_id);
    }

    #[test]
    fn it_serializes_and_deserializes_uuid_through_value_request_id() {
        let expected_id = ProgressToken::Uuid(uuid::Uuid::new_v4());

        let json = serde_json::to_value(&expected_id).unwrap();
        let new_id: ProgressToken = serde_json::from_value(json).unwrap();

        assert_eq!(expected_id, new_id);
    }
}