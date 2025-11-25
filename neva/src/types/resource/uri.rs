//! URI helpers and utilities

use serde::{Serialize, Deserialize};
use std::ops::{Deref, DerefMut};
use std::fmt::{Display, Formatter};
use crate::shared::MemChr;

const PATH_SEPARATOR: char = '/';
const SCHEME_SEPARATOR: [u8; 3] = [b':', b'/', b'/'];

/// Represents a resource URI
#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct Uri(String);

impl Deref for Uri {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        &self.0
    }
}

impl DerefMut for Uri {
    #[inline]
    fn deref_mut(&mut self) -> &mut str {
        &mut self.0
    }
}

impl Display for Uri {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for Uri {
    #[inline]
    fn from(s: String) -> Self {
        Uri(s)
    }
}

impl From<&str> for Uri {
    #[inline]
    fn from(s: &str) -> Self {
        Uri(s.to_owned())
    }
}

impl Uri {
    /// Returns the inner URL string
    #[inline]
    pub fn into_inner(self) -> String {
        self.0
    }
    
    /// Splits the URI into scheme and path parts
    /// 
    /// # Example
    /// ```rust
    /// use neva::types::Uri;
    /// 
    /// let uri = Uri::from("res://test1/test2");
    ///         
    /// assert_eq!(uri.parts().unwrap().collect::<Vec<_>>(), ["res", "test1", "test2"]);
    /// ```
    pub fn parts(&self) -> Option<impl Iterator<Item = &str>> {
        let scheme_end = memchr::memmem::find(self.as_bytes(), &SCHEME_SEPARATOR)?;
        let scheme = &self[..scheme_end];
        let mut rest = &self[scheme_end + 3..];

        rest = rest.trim_start_matches(PATH_SEPARATOR);

        Some(std::iter::once(scheme)
            .chain(MemChr::split(rest, PATH_SEPARATOR as u8)))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    
    #[test]
    fn it_converts_from_str() {
        let uri = Uri::from("res://test1");
        
        assert_eq!(uri.to_string(), "res://test1");
    }

    #[test]
    fn it_splits_scheme_and_path() {
        let uri = Uri::from("res://test1/test2");
        
        assert_eq!(uri.parts().unwrap().collect::<Vec<_>>(), ["res", "test1", "test2"]);
    }

    #[test]
    fn it_splits_empty() {
        let uri = Uri::from("");

        assert!(uri.parts().is_none());
    }

    #[test]
    fn it_splits_scheme_and_path_with_double_slash() {
        let uri = Uri::from("res://test1//test2");

        assert_eq!(uri.parts().unwrap().collect::<Vec<_>>(), ["res", "test1", "test2"]);
    }

    #[test]
    fn it_splits_scheme_and_path_with_trailing_slash() {
        let uri = Uri::from("res://test1/test2/");

        assert_eq!(uri.parts().unwrap().collect::<Vec<_>>(), ["res", "test1", "test2"]);
    }

    #[test]
    fn it_splits_scheme_and_path_with_leading_slash() {
        let uri = Uri::from("res:///test1/test2");

        assert_eq!(uri.parts().unwrap().collect::<Vec<_>>(), ["res", "test1", "test2"]);
    }
}