//! A set of helpers for types

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fmt::{self, Display, Formatter},
    ops::{Deref, DerefMut},
};

/// Represents a SchemaProperty type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PropertyType {
    Array,
    String,
    Number,
    Bool,
    Object,
}

impl From<&str> for PropertyType {
    #[inline]
    fn from(s: &str) -> Self {
        match s { 
            "array" => PropertyType::Array,
            "string" => PropertyType::String,
            "number" => PropertyType::Number,
            "bool" => PropertyType::Bool,
            "object" => PropertyType::Object,
            _ => PropertyType::Object,
        }
    }
}

impl From<String> for PropertyType {
    #[inline]
    fn from(s: String) -> Self {
        match s.as_str() {
            "array" => PropertyType::Array,
            "string" => PropertyType::String,
            "number" => PropertyType::Number,
            "bool" => PropertyType::Bool,
            "object" => PropertyType::Object,
            _ => PropertyType::Object,
        }
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
        }
    }
}

// Preventing conflicts
mod sealed {
    pub trait TypeCategorySealed {}
}

/// A traits that helps to determine a category of an object type
pub(crate) trait TypeCategory: sealed::TypeCategorySealed {
    fn category() -> PropertyType;
}

macro_rules! impl_type_category {
    ($t:ty, $cat:expr) => {
        impl sealed::TypeCategorySealed for $t {}
        impl TypeCategory for $t {
            #[inline]
            fn category() -> PropertyType {
                $cat
            }
        }
    };
    ($t:ty, $gt:ident, $cat:expr) => {
        impl<$gt> sealed::TypeCategorySealed for $t {}
        impl<$gt> TypeCategory for $t {
            #[inline]
            fn category() -> PropertyType {
                $cat
            }
        }
    };
}

// Simple types
impl_type_category!(String, PropertyType::String);
impl_type_category!(bool, PropertyType::Bool);

// Signed integer types
impl_type_category!(i8, PropertyType::Number);
impl_type_category!(i16, PropertyType::Number);
impl_type_category!(i32, PropertyType::Number);
impl_type_category!(i64, PropertyType::Number);
impl_type_category!(i128, PropertyType::Number);
impl_type_category!(isize, PropertyType::Number);

// Unsigned integer types
impl_type_category!(u8, PropertyType::Number);
impl_type_category!(u16, PropertyType::Number);
impl_type_category!(u32, PropertyType::Number);
impl_type_category!(u64, PropertyType::Number);
impl_type_category!(u128, PropertyType::Number);
impl_type_category!(usize, PropertyType::Number);

// Floating point numbers
impl_type_category!(f32, PropertyType::Number);
impl_type_category!(f64, PropertyType::Number);

// Array types
impl_type_category!(Vec<T>, T, PropertyType::Array);
impl_type_category!([T], T, PropertyType::Array);
impl_type_category!(&[T], T, PropertyType::Array);

/// Wraps JSON-typed data
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Json<T>(T);

impl<T> Json<T> {
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

impl<T: Display> Display for Json<T> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl<T> sealed::TypeCategorySealed for Json<T> {}
impl<T> TypeCategory for Json<T> {
    fn category() -> PropertyType {
        PropertyType::Object
    }
}

impl sealed::TypeCategorySealed for Value {}
impl TypeCategory for Value {
    fn category() -> PropertyType {
        PropertyType::Object
    }
}

#[cfg(test)]
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
