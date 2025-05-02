//! A set of helpers for types

use serde::{Deserialize, Serialize};
use std::{
    fmt::{self, Display, Formatter},
    ops::{Deref, DerefMut},
};

#[cfg(feature = "server")]
pub mod macros;
#[cfg(feature = "server")]
pub(crate) mod extract;

/// Represents a SchemaProperty type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PropertyType {
    None,
    Array,
    String,
    Number,
    #[serde(alias = "boolean")]
    Bool,
    Object,
}

impl Default for PropertyType {
    #[inline]
    fn default() -> Self {
        Self::Object
    }
}

impl From<&str> for PropertyType {
    #[inline]
    fn from(s: &str) -> Self {
        match s { 
            "array" => PropertyType::Array,
            "string" => PropertyType::String,
            "number" => PropertyType::Number,
            "bool" => PropertyType::Bool,
            "boolean" => PropertyType::Bool,
            "object" => PropertyType::Object,
            "none" => PropertyType::None,
            _ => PropertyType::Object,
        }
    }
}

impl From<String> for PropertyType {
    #[inline]
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl Display for PropertyType {
    #[inline]
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self { 
            PropertyType::Array => write!(f, "array"),
            PropertyType::String => write!(f, "string"),
            PropertyType::Number => write!(f, "number"),
            PropertyType::Bool => write!(f, "boolean"),
            PropertyType::Object => write!(f, "object"),
            PropertyType::None => write!(f, "none"),
        }
    }
}

// Preventing conflicts
#[cfg(feature = "server")]
mod sealed {
    pub trait TypeCategorySealed {}
}

/// A trait that helps to determine a category of an object type
#[cfg(feature = "server")]
pub(crate) trait TypeCategory: sealed::TypeCategorySealed {
    fn category() -> PropertyType;
}

/// Wraps JSON-typed data
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Json<T>(pub T);

/// Wraps  metadata
#[derive(Debug, Default)]
pub struct Meta<T>(pub T);

impl<T> Json<T> {
    /// Unwraps the inner `T`
    #[inline]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Meta<T> {
    /// Unwraps the inner `T`
    #[inline]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: Serialize> From<T> for Json<T> {
    #[inline]
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> Deref for Json<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Json<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T> Deref for Meta<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Meta<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: Display> Display for Json<T> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl<T: Display> Display for Meta<T> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod tests {
    use super::*;
    
    #[test]
    fn it_returns_category_for_string() {
        assert_eq!(String::category(), PropertyType::String);
    }

    #[test]
    fn it_returns_category_for_bool() {
        assert_eq!(bool::category(), PropertyType::Bool);
    }

    #[test]
    fn it_returns_category_for_i8() {
        assert_eq!(i8::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_i16() {
        assert_eq!(i16::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_i32() {
        assert_eq!(i32::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_i64() {
        assert_eq!(i64::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_i128() {
        assert_eq!(i128::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_isize() {
        assert_eq!(isize::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_u8() {
        assert_eq!(u8::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_u16() {
        assert_eq!(u16::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_u32() {
        assert_eq!(u32::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_u64() {
        assert_eq!(u64::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_u128() {
        assert_eq!(u128::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_usize() {
        assert_eq!(usize::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_f32() {
        assert_eq!(f32::category(), PropertyType::Number);
    }

    #[test]
    fn it_returns_category_for_f64() {
        assert_eq!(f64::category(), PropertyType::Number);
    }
    
    #[test]
    fn it_returns_category_for_json() {
        assert_eq!(Json::<Test>::category(), PropertyType::Object);
    }
    
    struct Test;
}
