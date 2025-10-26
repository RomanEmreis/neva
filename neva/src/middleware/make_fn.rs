//! Middleware factory functions

use std::{future::Future, sync::Arc};
use crate::{
    middleware::{Middleware, MwContext, Next},
    types::{Message, Response}
};

/// Turns a closure into middleware
#[inline]
pub(crate) fn make_mw<F, R>(f: F) -> Middleware
where
    F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
    R: Future<Output = Response> + Send + 'static,
{
    Arc::new(move |ctx: MwContext, next: Next| {
        Box::pin(f(ctx, next))
    })
}

/// Turns a closure into middleware that runs only 
/// if the MCP server received a message that satisfies the condition.
#[inline]
pub(super) fn make_on<F, P, R>(f: F, p: P)  -> Middleware
where
    F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
    P: Fn(&Message) -> bool  + Clone + Send + Sync + 'static,
    R: Future<Output = Response> + Send + 'static,
{
    let mw = move |ctx: MwContext, next: Next| {
        let f = f.clone();
        let p = p.clone();
        async move {
            if p(&ctx.msg) {
                f(ctx, next).await
            } else {
                next(ctx).await
            }
        }
    };
    make_mw(mw)
}

/// Turns a closure into middleware that runs only 
/// if the MCP server received a message that satisfies the condition.
#[inline]
pub(super) fn make_on_command<F, R>(f: F, command: &'static str)  -> Middleware
where
    F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
    R: Future<Output = Response> + Send + 'static,
{
    make_on(f, move |msg| {
        if let Message::Request(req) = msg {
            req.method == command
        } else {
            false
        }
    })
}