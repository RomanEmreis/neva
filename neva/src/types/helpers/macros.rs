//! Helper Macros

use super::{PropertyType, TypeCategory};
use crate::types::{
    CallToolRequestParams, GetPromptRequestParams, Json, Meta, ReadResourceRequestParams, Uri,
};
use serde_json::Value;

macro_rules! impl_type_category {
    ($t:ty, $cat:expr) => {
        impl super::sealed::TypeCategorySealed for $t {}
        impl TypeCategory for $t {
            #[inline]
            fn category() -> PropertyType {
                $cat
            }
        }
    };
    ($t:ty, $gt:ident, $cat:expr) => {
        impl<$gt> super::sealed::TypeCategorySealed for $t {}
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
impl_type_category!(Uri, PropertyType::String);
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

impl_type_category!(CallToolRequestParams, PropertyType::None);
impl_type_category!(ReadResourceRequestParams, PropertyType::None);
impl_type_category!(GetPromptRequestParams, PropertyType::None);
impl_type_category!(Meta<T>, T, PropertyType::None);
impl_type_category!(crate::Context, PropertyType::None);

// DI extractor: injected from the request context, never a tool/prompt argument.
// Implemented manually (not via `impl_type_category!`) because `Dc<T>` carries a
// `T: Send + Sync` bound the macro form cannot express.
#[cfg(feature = "di")]
impl<T: Send + Sync> super::sealed::TypeCategorySealed for crate::di::Dc<T> {}
#[cfg(feature = "di")]
impl<T: Send + Sync> TypeCategory for crate::di::Dc<T> {
    #[inline]
    fn category() -> PropertyType {
        PropertyType::None
    }
}

impl_type_category!(Value, PropertyType::Object);
impl_type_category!(Json<T>, T, PropertyType::Object);
