//! Utilities for conversion various types into tool or prompt arguments

use std::collections::HashMap;
use serde::Serialize;

/// A trait describes arguments for tools and prompts
pub trait IntoArgs {
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>>;
}

impl IntoArgs for () {
    #[inline]
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>> {
        None
    }
}

impl<T: IntoArgs> IntoArgs for Option<T> {
    #[inline]
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>> {
        self.and_then(|args| args.into_args())
    }
}

impl<K, T> IntoArgs for (K, T)
where
    K: Into<String>,
    T: Serialize,
{
    #[inline]
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>> {
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
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>> {
        Some(make_args(self))
    }
}

impl<K, T> IntoArgs for Vec<(K, T)>
where
    K: Into<String>,
    T: Serialize,
{
    #[inline]
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>> {
        Some(make_args(self))
    }
}

impl<K, T> IntoArgs for HashMap<K, T>
where
    K: Into<String>,
    T: Serialize,
{
    #[inline]
    fn into_args(self) -> Option<HashMap<String, serde_json::Value>> {
        Some(make_args(self))
    }
}

/// Creates arguments for tools and prompts from iterator
#[inline]
fn make_args<I, K, T>(args: I) -> HashMap<String, serde_json::Value>
where
    I: IntoIterator<Item = (K, T)>,
    K: Into<String>,
    T: Serialize,
{
    HashMap::from_iter(args
        .into_iter()
        .map(|(k, v)| (k.into(), serde_json::to_value(v).unwrap())))
}