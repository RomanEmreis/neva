//! Extractors for Dependency Injection

use crate::error::{Error, ErrorCode};
use crate::app::handler::{HandlerParams, FromHandlerParams};
use crate::types::{helpers::extract::RequestArgument, resource::ResourceArgument, RequestParamsMeta};
use std::{
    ops::{Deref, DerefMut},
    sync::Arc
};

/// `Dc` stands for Dependency Container.  
/// This struct wraps an injectable type `T` that is **shared** between all handlers
/// through an [`Arc`].
///
/// # Example
/// ```no_run
/// 
/// ```
#[derive(Debug, Clone)]
pub struct Dc<T: Send + Sync>(Arc<T>);

impl<T: Send + Sync> Deref for Dc<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone + Send + Sync> DerefMut for Dc<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}

impl<T: Send + Sync> Dc<T> {
    /// Unwraps the inner [`Arc`]
    #[inline]
    pub fn into_inner(self) -> Arc<T> {
        self.0
    }
}

impl<T: Send + Sync + Clone> Dc<T> {
    /// Clones and returns the inner `T`.
    ///
    /// Equivalent to calling [`Clone::clone`] on the inner `T`.
    #[inline]
    pub fn cloned(&self) -> T {
        self.0.as_ref().clone()
    }
}

impl<T: Send + Sync + 'static> ResourceArgument for Dc<T> {
    type Error = Error;

    #[inline]
    fn extract(payload: crate::types::resource::Payload<'_>) -> Result<Self, Self::Error> {
        make_dc(payload.expect_meta().as_ref())
    }

    #[inline]
    fn source() -> crate::types::resource::Source {
        crate::types::resource::Source::Meta
    }
}

impl<T: Send + Sync + 'static> RequestArgument for Dc<T> {
    type Error = Error;

    #[inline]
    fn extract(payload: crate::types::helpers::extract::Payload<'_>) -> Result<Self, Self::Error> {
        make_dc(payload.expect_meta().as_ref())
    }

    #[inline]
    fn source() -> crate::types::helpers::extract::Source {
        crate::types::helpers::extract::Source::Meta
    }
}

impl<T: Send + Sync + 'static> FromHandlerParams for Dc<T> {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        match params {
            HandlerParams::Request(context, _) => context.resolve_shared().map(Dc),
            _ => Err(Error::new(ErrorCode::InternalError, "invalid handler parameters"))
        }
    }
}

#[inline(always)]
fn make_dc<T: Send + Sync + 'static>(meta: Option<&RequestParamsMeta>) -> Result<Dc<T>, Error> {
    meta.and_then(|meta| meta.context.as_ref())
        .ok_or(Error::new(ErrorCode::InvalidParams, "Missing MCP context"))?
        .resolve_shared()
        .map(Dc)
}