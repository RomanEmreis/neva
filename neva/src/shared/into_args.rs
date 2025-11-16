//! Utilities for conversion various types into tool or prompt arguments

use crate::types::Json;
use std::collections::HashMap;
use serde::Serialize;
use serde_json::Value;

/// A trait describes arguments for tools and prompts
pub trait IntoArgs {
    /// Converts self into arguments for tools and prompts
    fn into_args(self) -> Option<HashMap<String, Value>>;
}

impl IntoArgs for () {
    #[inline]
    fn into_args(self) -> Option<HashMap<String, Value>> {
        None
    }
}

impl<T: IntoArgs> IntoArgs for Option<T> {
    #[inline]
    fn into_args(self) -> Option<HashMap<String, Value>> {
        self.and_then(|args| args.into_args())
    }
}

impl<K, T> IntoArgs for (K, T)
where
    K: Into<String>,
    T: Serialize,
{
    #[inline]
    fn into_args(self) -> Option<HashMap<String, Value>> {
        Some(HashMap::from([
            (self.0.into(), serde_json::to_value(self.1).unwrap())
        ]))
    }
}

impl<K, T, const N: usize> IntoArgs for [(K, T); N]
where
    K: Into<String>,
    T: Serialize,
{
    #[inline]
    fn into_args(self) -> Option<HashMap<String, Value>> {
        Some(make_args(self))
    }
}

impl<K, T> IntoArgs for Vec<(K, T)>
where
    K: Into<String>,
    T: Serialize,
{
    #[inline]
    fn into_args(self) -> Option<HashMap<String, Value>> {
        Some(make_args(self))
    }
}

impl<K, T> IntoArgs for HashMap<K, T>
where
    K: Into<String>,
    T: Serialize,
{
    #[inline]
    fn into_args(self) -> Option<HashMap<String, Value>> {
        Some(make_args(self))
    }
}

impl IntoArgs for Value {
    #[inline]
    fn into_args(self) -> Option<HashMap<String, Value>> {
        match self {
            Value::Object(map) => Some(map
                .into_iter()
                .collect()),
            _ => None,
        }
    }
}

impl<T: Serialize> IntoArgs for Json<T> {
    #[inline]
    fn into_args(self) -> Option<HashMap<String, Value>> {
        serde_json::to_value(self.0)
            .ok()
            .into_args()
    }
} 

/// Creates arguments for tools and prompts from iterator
#[inline]
fn make_args<I, K, T>(args: I) -> HashMap<String, Value>
where
    I: IntoIterator<Item = (K, T)>,
    K: Into<String>,
    T: Serialize,
{
    HashMap::from_iter(args
        .into_iter()
        .map(|(k, v)| (k.into(), serde_json::to_value(v).unwrap())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn it_returns_none_for_unit_type() {
        let args = ().into_args();
        assert!(args.is_none());
    }

    #[test]
    fn it_returns_none_for_none_option() {
        let args: Option<(String, i32)> = None;
        let result = args.into_args();
        assert!(result.is_none());
    }

    #[test]
    fn it_converts_some_option_into_hashmap() {
        let args = Some(("key", 123)).into_args().unwrap();
        assert_eq!(args.get("key"), Some(&json!(123)));
    }

    #[test]
    fn it_converts_tuple_into_single_key_value_pair() {
        let args = ("answer", 42)
            .into_args()
            .unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args.get("answer"), Some(&json!(42)));
    }

    #[test]
    fn it_converts_array_of_pairs_into_hashmap() {
        let args = [("a", 1), ("b", 2)]
            .into_args()
            .unwrap();
        assert_eq!(args.get("a"), Some(&json!(1)));
        assert_eq!(args.get("b"), Some(&json!(2)));
    }

    #[test]
    fn it_converts_vec_of_pairs_into_hashmap() {
        let args = vec![("x", true), ("y", false)]
            .into_args()
            .unwrap();
        assert_eq!(args.get("x"), Some(&json!(true)));
        assert_eq!(args.get("y"), Some(&json!(false)));
    }

    #[test]
    fn it_converts_hashmap_into_hashmap_of_json_values() {
        let mut map = HashMap::new();
        map.insert("one", 1);
        map.insert("two", 2);

        let args = map.into_args().unwrap();
        assert_eq!(args.get("one"), Some(&json!(1)));
        assert_eq!(args.get("two"), Some(&json!(2)));
    }

    #[test]
    fn it_handles_string_and_number_mixed_types() {
        let args = vec![("name", json!("Alice")), ("age", json!(30))]
            .into_args()
            .unwrap();
        assert_eq!(args.get("name"), Some(&json!("Alice")));
        assert_eq!(args.get("age"), Some(&json!(30)));
    }

    #[test]
    fn it_overwrites_duplicate_keys_in_later_entries() {
        let args = vec![("k", 1), ("k", 2)]
            .into_args()
            .unwrap();
        assert_eq!(args.get("k"), Some(&json!(2)));
    }

    #[test]
    fn it_supports_keys_that_are_not_str_directly() {
        let args = vec![(String::from("id"), 99)]
            .into_args()
            .unwrap();
        assert_eq!(args.get("id"), Some(&json!(99)));
    }

    #[test]
    fn it_creates_empty_hashmap_for_empty_vec() {
        let args: Vec<(String, i32)> = vec![];
        let result = args
            .into_args()
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn it_handles_serde_value() {
        let args = json!({ "name": "Alice", "age": 30  })
            .into_args()
            .unwrap();
        assert_eq!(args.get("name"), Some(&json!("Alice")));
        assert_eq!(args.get("age"), Some(&json!(30)));
    }

    #[test]
    fn it_handles_json_value() {
        let args = Json(User { name: "Alice".into(), age: 30 })
            .into_args()
            .unwrap();
        assert_eq!(args.get("name"), Some(&json!("Alice")));
        assert_eq!(args.get("age"), Some(&json!(30)));
    }
    
    #[derive(Serialize)]
    struct User {
        name: String,
        age: i32,
    }
}
