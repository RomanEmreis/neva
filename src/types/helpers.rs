//! A set of helpers for types

use serde::{Deserialize, Serialize};
use std::{
    fmt::{self, Display, Formatter},
    ops::{Deref, DerefMut},
};

// Preventing conflicts
mod sealed {
    pub trait TypeCategorySealed {}
}

pub(crate) trait TypeCategory: sealed::TypeCategorySealed {
    fn category() -> &'static str;
}

macro_rules! impl_type_category {
    ($t:ty, $cat:expr) => {
        impl sealed::TypeCategorySealed for $t {}
        impl TypeCategory for $t {
            #[inline]
            fn category() -> &'static str {
                $cat
            }
        }
    };
    ($t:ty, $gt:ident, $cat:expr) => {
        impl<$gt> sealed::TypeCategorySealed for $t {}
        impl<$gt> TypeCategory for $t {
            #[inline]
            fn category() -> &'static str {
                $cat
            }
        }
    };
}

// Simple types
impl_type_category!(String, "string");
impl_type_category!(bool, "boolean");

// Signed integer types
impl_type_category!(i8, "number");
impl_type_category!(i16, "number");
impl_type_category!(i32, "number");
impl_type_category!(i64, "number");
impl_type_category!(i128, "number");
impl_type_category!(isize, "number");

// Unsigned integer types
impl_type_category!(u8, "number");
impl_type_category!(u16, "number");
impl_type_category!(u32, "number");
impl_type_category!(u64, "number");
impl_type_category!(u128, "number");
impl_type_category!(usize, "number");

// Floating point numbers
impl_type_category!(f32, "number");
impl_type_category!(f64, "number");

// Array types
impl_type_category!(Vec<T>, T, "number");
impl_type_category!([T], T, "number");
impl_type_category!(&[T], T, "number");

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
    fn category() -> &'static str {
        "object"
    }
}

impl sealed::TypeCategorySealed for serde_json::Value {}
impl TypeCategory for serde_json::Value {
    fn category() -> &'static str {
        "object"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn it_returns_category_for_string() {
        assert_eq!(String::category(), "string");
    }

    #[test]
    fn it_returns_category_for_bool() {
        assert_eq!(bool::category(), "boolean");
    }

    #[test]
    fn it_returns_category_for_i8() {
        assert_eq!(i8::category(), "number");
    }

    #[test]
    fn it_returns_category_for_i16() {
        assert_eq!(i16::category(), "number");
    }

    #[test]
    fn it_returns_category_for_i32() {
        assert_eq!(i32::category(), "number");
    }

    #[test]
    fn it_returns_category_for_i64() {
        assert_eq!(i64::category(), "number");
    }

    #[test]
    fn it_returns_category_for_i128() {
        assert_eq!(i128::category(), "number");
    }

    #[test]
    fn it_returns_category_for_isize() {
        assert_eq!(isize::category(), "number");
    }

    #[test]
    fn it_returns_category_for_u8() {
        assert_eq!(u8::category(), "number");
    }

    #[test]
    fn it_returns_category_for_u16() {
        assert_eq!(u16::category(), "number");
    }

    #[test]
    fn it_returns_category_for_u32() {
        assert_eq!(u32::category(), "number");
    }

    #[test]
    fn it_returns_category_for_u64() {
        assert_eq!(u64::category(), "number");
    }

    #[test]
    fn it_returns_category_for_u128() {
        assert_eq!(u128::category(), "number");
    }

    #[test]
    fn it_returns_category_for_usize() {
        assert_eq!(usize::category(), "number");
    }

    #[test]
    fn it_returns_category_for_f32() {
        assert_eq!(f32::category(), "number");
    }

    #[test]
    fn it_returns_category_for_f64() {
        assert_eq!(f64::category(), "number");
    }
    
    #[test]
    fn it_returns_category_for_json() {
        assert_eq!(Json::<Test>::category(), "object");
    }
    
    struct Test;
}
