//! Traits and helpers for type extraction from request arguments

use crate::Context;
use crate::error::{Error, ErrorCode};
use crate::shared::{ArcSlice, ArcStr};
use crate::types::helpers::TypeCategory;
use crate::types::request::RequestParamsMeta;
use crate::types::{Meta, ProgressToken};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;

#[cfg(feature = "tasks")]
use crate::types::RelatedTaskMetadata;

/// Fallback names for a handler whose argument names are not known.
///
/// A handler accepts at most five value-carrying arguments, so the table is
/// never indexed past its end; the extra slots only keep [`ArgNames::get`]
/// total.
const POSITIONAL: [&str; 8] = [
    "arg0", "arg1", "arg2", "arg3", "arg4", "arg5", "arg6", "arg7",
];

/// The fallback name of the `index`-th argument slot.
#[inline]
pub(crate) fn positional_name(index: usize) -> &'static str {
    match POSITIONAL.get(index) {
        Some(name) => name,
        None => "",
    }
}

/// The slot `name` is the fallback name of, if it is one.
#[inline]
pub(crate) fn positional_slot(name: &str) -> Option<usize> {
    POSITIONAL.iter().position(|positional| *positional == name)
}

/// The names of a tool or prompt handler's arguments, in declaration order.
///
/// Arguments are read from the request's `arguments` map **by name**: the
/// *n*-th value-carrying parameter of the handler is looked up under the
/// *n*-th name here. Parameters that come from request metadata instead --
/// [`Meta`], [`Context`], or a DI-injected `Dc<T>` -- carry no name and do not
/// consume a slot.
///
/// The names are also exactly the property names the handler's generated
/// `inputSchema` (or prompt `arguments` list) advertises, which is what keeps
/// what a peer is told to send and what the handler reads from drifting apart.
///
/// A handler registered without declared names -- a bare closure passed to
/// [`crate::App::map_tool`], whose parameter names Rust does not preserve --
/// falls back to the positional `arg0`, `arg1`, ... form, and its generated
/// schema advertises those same names.
///
/// # Examples
///
/// ```
/// use neva::types::ArgNames;
///
/// let names = ArgNames::new(["age", "name"]);
/// assert_eq!(names.get(0), "age");
/// assert_eq!(names.get(1), "name");
///
/// // Undeclared, or past the end of what was declared: positional fallback.
/// assert_eq!(ArgNames::default().get(1), "arg1");
/// assert_eq!(names.get(2), "arg2");
/// ```
#[derive(Debug, Default, Clone)]
pub struct ArgNames {
    /// The declared names, if the handler's are known.
    names: Option<ArcSlice<ArcStr>>,
    /// How many argument slots the handler has, named or not.
    arity: usize,
}

impl ArgNames {
    /// Creates [`ArgNames`] from the handler's declared argument names.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::types::ArgNames;
    ///
    /// let names = ArgNames::new(["age", "name"]);
    /// assert_eq!(names.len(), 2);
    /// ```
    #[inline]
    pub fn new<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let names = names
            .into_iter()
            .map(|name| ArcStr::from(name.into()))
            .collect::<Vec<_>>();
        Self {
            arity: names.len(),
            names: Some(ArcSlice::from(names)),
        }
    }

    /// Creates undeclared [`ArgNames`] for a handler with `arity` argument
    /// slots, which read the positional `arg0`, `arg1`, ... names.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::types::ArgNames;
    ///
    /// let names = ArgNames::positional(2);
    /// assert_eq!(names.get(0), "arg0");
    /// assert!(!names.is_declared());
    /// ```
    #[inline]
    pub fn positional(arity: usize) -> Self {
        Self { names: None, arity }
    }

    /// Returns `true` when the handler's own argument names are known, rather
    /// than the positional fallback being in use.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::types::ArgNames;
    ///
    /// assert!(ArgNames::new(["city"]).is_declared());
    /// assert!(!ArgNames::positional(1).is_declared());
    /// ```
    #[inline]
    pub fn is_declared(&self) -> bool {
        self.names.is_some()
    }

    /// Returns how many argument slots the handler has, named or not.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::types::ArgNames;
    ///
    /// assert_eq!(ArgNames::positional(3).arity(), 3);
    /// assert_eq!(ArgNames::new(["a", "b"]).arity(), 2);
    /// ```
    #[inline]
    pub fn arity(&self) -> usize {
        self.arity
    }

    /// Returns the name of the argument in the `index`-th slot, falling back
    /// to the positional `argN` form when no name was declared for it.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::types::ArgNames;
    ///
    /// assert_eq!(ArgNames::new(["city"]).get(0), "city");
    /// assert_eq!(ArgNames::default().get(0), "arg0");
    /// ```
    #[inline]
    pub fn get(&self, index: usize) -> &str {
        self.names
            .as_deref()
            .and_then(|names| names.get(index))
            .map_or_else(|| positional_name(index), |name| &**name)
    }

    /// Returns the number of declared names, or `0` if none were declared.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::types::ArgNames;
    ///
    /// assert_eq!(ArgNames::new(["a", "b"]).len(), 2);
    /// assert_eq!(ArgNames::default().len(), 0);
    /// ```
    #[inline]
    pub fn len(&self) -> usize {
        self.names.as_deref().map_or(0, <[ArcStr]>::len)
    }

    /// Declares `names` for the handler these [`ArgNames`] describe, keeping
    /// the handler's own arity so a miscounted declaration stays detectable.
    #[inline]
    pub(crate) fn declare<I, S>(&self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            arity: self.arity,
            ..Self::new(names)
        }
    }

    /// Returns `true` when no names were declared.
    ///
    /// # Examples
    ///
    /// ```
    /// use neva::types::ArgNames;
    ///
    /// assert!(ArgNames::default().is_empty());
    /// assert!(!ArgNames::new(["a"]).is_empty());
    /// ```
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Represents a payload that needs the type to be extracted from
pub(crate) enum Payload<'a> {
    /// Tool or Prompt argument
    Args(serde_json::Value),

    /// Request metadata ("_meta")
    Meta(&'a Option<RequestParamsMeta>),
}

/// Represents an extraction sources
pub(crate) enum Source {
    /// Tool or Prompt arguments
    Args,
    /// Request metadata ("_meta")
    Meta,
}

/// A trait that type needs to implement to be extractable from [`crate::types::Request`]
pub(crate) trait RequestArgument: Sized {
    type Error;

    /// Extracts a type value from [`Payload`]
    fn extract(payload: Payload<'_>) -> Result<Self, Self::Error>;

    /// Returns a [`Source`] that the type needs to be extracted from
    #[inline]
    fn source() -> Source {
        Source::Args
    }
}

/// The parts of a request's params that argument extraction reads.
///
/// Implemented by `tools/call` and `prompts/get` params so both share one set
/// of tuple extraction impls.
pub(crate) trait HandlerArgs {
    /// Splits the params into its `arguments` map and its `_meta`.
    fn into_parts(self) -> (Option<HashMap<String, Value>>, Option<RequestParamsMeta>);
}

/// Builds a handler's argument tuple from the params of a `tools/call` or
/// `prompts/get` request.
///
/// This is the extraction counterpart of [`ArgNames`]: `P` carries the values
/// a peer sent, `names` says which name each handler slot reads.
///
/// # Examples
///
/// ```
/// use neva::types::{ArgNames, CallToolRequestParams, FromHandlerArgs};
/// use serde_json::json;
///
/// // A peer may send the arguments in any order -- extraction is by name.
/// let params = CallToolRequestParams::new("greet")
///     .with_args([("age", json!(30)), ("name", json!("John"))]);
///
/// let (name, age) = <(String, i32)>::from_args(params, &ArgNames::new(["name", "age"]))?;
///
/// assert_eq!(name, "John");
/// assert_eq!(age, 30);
/// # Ok::<(), neva::error::Error>(())
/// ```
pub trait FromHandlerArgs<P>: Sized {
    /// Extracts the handler's arguments from `params`.
    fn from_args(params: P, names: &ArgNames) -> Result<Self, Error>;
}

impl<'a> Payload<'a> {
    /// Returns arguments value for type extraction
    #[inline]
    pub(crate) fn expect_args(self) -> serde_json::Value {
        match self {
            Payload::Args(val) => val,
            _ => unreachable!("Expected Args variant"),
        }
    }

    /// Returns an optional [`RequestParamsMeta`] for type extraction
    #[inline]
    pub(crate) fn expect_meta(self) -> &'a Option<RequestParamsMeta> {
        match self {
            Payload::Meta(meta) => meta,
            _ => unreachable!("Expected Meta variant"),
        }
    }
}

impl<T: DeserializeOwned> RequestArgument for T {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload<'_>) -> Result<Self, Self::Error> {
        let arg = payload.expect_args();
        T::deserialize(arg).map_err(Error::from)
    }
}

impl RequestArgument for Meta<RequestParamsMeta> {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload<'_>) -> Result<Self, Self::Error> {
        let meta = payload.expect_meta();
        meta.clone()
            .ok_or(Error::new(ErrorCode::InvalidParams, "Missing metadata"))
            .map(Meta)
    }

    #[inline]
    fn source() -> Source {
        Source::Meta
    }
}

impl RequestArgument for Meta<ProgressToken> {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload<'_>) -> Result<Self, Self::Error> {
        let meta = payload.expect_meta();
        meta.as_ref()
            .and_then(|meta| meta.progress_token.clone())
            .ok_or(Error::new(
                ErrorCode::InvalidParams,
                "Missing progress token",
            ))
            .map(Meta)
    }

    #[inline]
    fn source() -> Source {
        Source::Meta
    }
}

#[cfg(feature = "tasks")]
impl RequestArgument for Meta<RelatedTaskMetadata> {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload<'_>) -> Result<Self, Self::Error> {
        let meta = payload.expect_meta();
        meta.as_ref()
            .and_then(|meta| meta.task.clone())
            .ok_or(Error::new(
                ErrorCode::InvalidParams,
                "Missing progress token",
            ))
            .map(Meta)
    }

    #[inline]
    fn source() -> Source {
        Source::Meta
    }
}

impl RequestArgument for Context {
    type Error = Error;

    #[inline]
    fn extract(payload: Payload<'_>) -> Result<Self, Self::Error> {
        let meta = payload.expect_meta();
        meta.as_ref()
            .and_then(|meta| meta.context.clone())
            .ok_or(Error::new(ErrorCode::InvalidParams, "Missing MCP context"))
    }

    #[inline]
    fn source() -> Source {
        Source::Meta
    }
}

/// Extracts one handler argument.
///
/// Metadata-sourced types read `meta` and leave `slot` alone; everything else
/// consumes the next name and reads the value a peer sent under it. An absent
/// key is offered to the type as `null`, so an `Option<T>` argument resolves
/// to `None` instead of failing.
///
/// Whether an argument may be omitted is decided by its *type*
/// ([`TypeCategory::is_optional`]), never by whether a synthetic `null`
/// happens to deserialize into it: `serde_json::Value` and `()` accept `null`
/// quite happily, and inferring optionality from that would let a required
/// argument through as `Null` against the schema that declares it required.
#[inline]
pub(crate) fn extract_arg<T: RequestArgument<Error = Error> + TypeCategory>(
    meta: &Option<RequestParamsMeta>,
    args: Option<&HashMap<String, Value>>,
    names: &ArgNames,
    slot: &mut usize,
) -> Result<T, Error> {
    match T::source() {
        Source::Meta => T::extract(Payload::Meta(meta)),
        Source::Args => {
            let name = names.get(*slot);
            *slot += 1;
            match args.and_then(|args| args.get(name)) {
                Some(value) => T::extract(Payload::Args(value.clone())).map_err(|err| {
                    Error::new(
                        ErrorCode::InvalidParams,
                        format!("invalid value for argument `{name}`: {err}"),
                    )
                }),
                None if T::is_optional() => T::extract(Payload::Args(Value::Null)),
                None => Err(Error::new(
                    ErrorCode::InvalidParams,
                    format!("missing required argument `{name}`"),
                )),
            }
        }
    }
}

impl<P: HandlerArgs> FromHandlerArgs<P> for () {
    #[inline]
    fn from_args(_: P, _: &ArgNames) -> Result<Self, Error> {
        Ok(())
    }
}

macro_rules! impl_from_handler_args {
    ($($T:ident),+) => {
        impl<P: HandlerArgs, $($T: RequestArgument<Error = Error> + TypeCategory),+> FromHandlerArgs<P> for ($($T,)+) {
            #[inline]
            fn from_args(params: P, names: &ArgNames) -> Result<Self, Error> {
                let (args, meta) = params.into_parts();
                let args = args.as_ref();
                // Tuple elements are evaluated left to right, so `slot` walks
                // the declared names in the handler's own parameter order.
                let mut slot = 0;
                let tuple = (
                    $(
                        extract_arg::<$T>(&meta, args, names, &mut slot)?,
                    )+
                );
                Ok(tuple)
            }
        }
    };
}

impl_from_handler_args! { T1 }
impl_from_handler_args! { T1, T2 }
impl_from_handler_args! { T1, T2, T3 }
impl_from_handler_args! { T1, T2, T3, T4 }
impl_from_handler_args! { T1, T2, T3, T4, T5 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_returns_declared_names() {
        let names = ArgNames::new(["age", "name"]);

        assert_eq!(names.get(0), "age");
        assert_eq!(names.get(1), "name");
        assert_eq!(names.len(), 2);
        assert!(!names.is_empty());
    }

    #[test]
    fn it_falls_back_to_positional_names() {
        let names = ArgNames::default();

        assert_eq!(names.get(0), "arg0");
        assert_eq!(names.get(4), "arg4");
        assert!(names.is_empty());
    }

    #[test]
    fn it_falls_back_past_the_declared_names() {
        let names = ArgNames::new(["age"]);

        assert_eq!(names.get(0), "age");
        assert_eq!(names.get(1), "arg1");
    }
}
