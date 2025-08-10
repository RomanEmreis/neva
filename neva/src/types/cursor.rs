//! Cursor-based pagination utilities

use std::ops::{Deref, DerefMut};
use serde::{Serialize, Serializer, Deserialize, Deserializer};
use base64::{engine::general_purpose, Engine as _};

/// An opaque token representing the pagination position after the last returned result.
#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct Cursor(pub usize);

impl Serialize for Cursor {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize the usize as JSON, then base64 encode it
        let json = serde_json::to_vec(&self.0).map_err(serde::ser::Error::custom)?;
        let encoded = general_purpose::STANDARD.encode(json);
        serializer.serialize_str(&encoded)
    }
}

impl<'de> Deserialize<'de> for Cursor {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let decoded = general_purpose::STANDARD
            .decode(&encoded)
            .map_err(serde::de::Error::custom)?;
        
        let index: usize =
            serde_json::from_slice(&decoded).map_err(serde::de::Error::custom)?;
        
        Ok(Cursor(index))
    }
}

impl Deref for Cursor {
    type Target = usize;
    
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Cursor {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Represents a current page of items
pub struct Page<'a, T> {
    /// Page items
    pub items: &'a [T],

    /// An opaque token representing the pagination position after the last returned result.
    pub next_cursor: Option<Cursor>,
}

/// A trait for types that need pagination support
pub trait Pagination<T> {
    fn paginate(&self, cursor: Option<Cursor>, page_size: usize) -> Page<'_, T>;
}

impl<T> Pagination<T> for Vec<T> {
    #[inline]
    fn paginate(&self, cursor: Option<Cursor>, page_size: usize) -> Page<'_, T> {
        self.as_slice().paginate(cursor, page_size)
    }
}

impl<T> Pagination<T> for [T] {
    #[inline]
    fn paginate(&self, cursor: Option<Cursor>, page_size: usize) -> Page<'_, T> {
        let start = *cursor.unwrap_or_default();
        let end = usize::min(start + page_size, self.len());

        let items = &self[start..end];
        let next_cursor = if end < self.len() {
            Some(Cursor(end))
        } else {
            None
        };

        Page { items, next_cursor }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_serializes_cursor() {
        let cursor = Cursor(42);
        let json = serde_json::to_string(&cursor).unwrap();

        // Ensure the result is a string
        assert!(json.starts_with("\"") && json.ends_with("\""));

        // Decode manually to validate correctness
        let base64_str = json.trim_matches('"');
        let decoded = general_purpose::STANDARD.decode(base64_str).unwrap();
        let index: usize = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(index, 42);
    }

    #[test]
    fn it_deserializes_cursor() {
        let cursor = Cursor(123456);
        let json = serde_json::to_string(&cursor).unwrap();

        let parsed: Cursor = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, cursor);
    }

    #[test]
    fn it_does_roundtrip() {
        for i in [0, 1, 42, 9999, usize::MAX / 2] {
            let original = Cursor(i);
            let json = serde_json::to_string(&original).unwrap();
            let decoded: Cursor = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, original);
        }
    }

    #[test]
    fn it_returns_invalid_base64() {
        let result: Result<Cursor, _> = serde_json::from_str("\"not_base64\"");
        assert!(result.is_err());
    }

    #[test]
    fn it_returns_invalid_json_inside_base64() {
        // base64 of "not_json" (just bytes, not valid JSON)
        let invalid = general_purpose::STANDARD.encode(b"not_json");
        let json = format!("\"{}\"", invalid);
        let result: Result<Cursor, _> = serde_json::from_str(&json);
        assert!(result.is_err());
    }

    #[test]
    fn it_paginates_over_vec() {
        let data = vec![1, 2, 3, 4, 5];
        let mut cursor = None;
        let mut collected = vec![];

        loop {
            let page = data.paginate(cursor, 2);
            collected.extend_from_slice(page.items);
            cursor = page.next_cursor;
            if cursor.is_none() { break; }
        }

        assert_eq!(collected, data);
    }
}