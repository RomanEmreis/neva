//! Serializable [`Arc<str>`]

use std::fmt;
use std::sync::Arc;
use std::ops::Deref;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArcStr(Arc<str>);

impl fmt::Display for ArcStr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl Deref for ArcStr {
    type Target = str;
    
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<String> for ArcStr {
    #[inline]
    fn from(s: String) -> Self {
        Self(Arc::from(s))
    }
}

impl From<&str> for ArcStr {
    #[inline]
    fn from(s: &str) -> Self {
        Self(Arc::from(s))
    }
}

impl From<Arc<str>> for ArcStr {
    #[inline]
    fn from(s: Arc<str>) -> Self {
        Self(s)
    }
}

impl Serialize for ArcStr {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where 
        S: serde::Serializer
    {
        serializer.serialize_str(self)
    }
}

impl<'de> Deserialize<'de> for ArcStr {
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de> 
    {
        let s: &str = Deserialize::deserialize(deserializer)?;
        Ok(ArcStr::from(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct Wrapper {
        id: ArcStr,
    }

    #[test]
    fn serialize_arcstr() {
        let w = Wrapper { id: ArcStr::from("hello") };
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(json, r#"{"id":"hello"}"#);
    }

    #[test]
    fn deserialize_arcstr() {
        let json = r#"{"id":"world"}"#;
        let w: Wrapper = serde_json::from_str(json).unwrap();
        assert_eq!(w.id.deref(), "world");
    }

    #[test]
    fn display_arcstr() {
        let s = ArcStr::from("test");
        assert_eq!(format!("{}", s), "test");
    }

    #[test]
    fn debug_arcstr() {
        let s = ArcStr::from("test");
        assert_eq!(format!("{:?}", s), r#"ArcStr("test")"#);
    }

    #[test]
    fn arc_clone_is_shared() {
        let s1 = ArcStr::from("shared");
        let s2 = s1.clone();
        assert!(Arc::ptr_eq(&s1.0, &s2.0));
    }

    #[test]
    fn equality_and_hash() {
        use std::collections::HashSet;

        let a = ArcStr::from("id-123");
        let b = ArcStr::from("id-123");
        let mut set = HashSet::new();
        set.insert(a.clone());

        assert_eq!(a, b);
        assert!(set.contains(&b));
    }
}
