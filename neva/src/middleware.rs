//! MCP Server middleware utilities

use std::sync::Arc;
use futures_util::future::BoxFuture;
use crate::{
    app::context::ServerRuntime, 
    types::{Message, RequestId, Request, Response, notification::Notification}
};

pub(super) mod make_fn;
pub mod wrap;

const DEFAULT_MW_CAPACITY: usize = 8;

/// Current middleware operation context.
pub struct MwContext {
    pub msg: Message,
    pub(super) runtime: ServerRuntime
}

/// A reference to the next middleware in the chain
pub type Next = Arc<
    dyn Fn(MwContext) -> BoxFuture<'static, Response>
    + Send 
    + Sync
>;

/// Middleware function wrapper
pub(super) type Middleware = Arc<
    dyn Fn(MwContext, Next) -> BoxFuture<'static, Response>
    + Send
    + Sync
>;

/// MCP middleware pipeline.
#[derive(Clone)]
pub(super) struct Middlewares {
    pub(super) pipeline: Vec<Middleware>
}

impl MwContext {
    /// Creates a new middleware message context
    #[inline]
    pub(super) fn msg(msg: Message, runtime: ServerRuntime) -> Self {
        Self { msg, runtime }
    }

    /// Returns current MCP [`Message`] ID
    #[inline]
    pub fn id(&self) -> RequestId {
        self.msg.id()
    }

    /// Returns current MCP session ID
    #[inline]
    pub fn session_id(&self) -> Option<&uuid::Uuid> {
        self.msg.session_id()
    }

    /// If the current message type is [`Request`] returns a reference to it,
    /// otherwise returns `None`
    #[inline]
    pub fn request(&self) -> Option<&Request> {
        if let Message::Request(req) = &self.msg {
            Some(req)
        } else {
            None
        }
    }

    /// If the current message type is [`Request`] returns a mutable reference to it,
    /// otherwise returns `None`
    #[inline]
    pub fn request_mut(&mut self) -> Option<&mut Request> {
        if let Message::Request(req) = &mut self.msg {
            Some(req)
        } else {
            None
        }
    }

    /// If the current request type is [`Response`] returns a reference to it,
    /// otherwise returns `None`
    #[inline]
    pub fn response(&self) -> Option<&Response> {
        if let Message::Response(resp) = &self.msg {
            Some(resp)
        } else {
            None
        }
    }

    /// If the current request type is [`Response`] returns a mutable reference to it,
    /// otherwise returns `None`
    #[inline]
    pub fn response_mut(&mut self) -> Option<&mut Response> {
        if let Message::Response(resp) = &mut self.msg {
            Some(resp)
        } else {
            None
        }
    }

    /// If the current request type is [`Notification`] returns a reference to it,
    /// otherwise returns `None`
    #[inline]
    pub fn notification(&self) -> Option<&Notification> {
        if let Message::Notification(notify) = &self.msg {
            Some(notify)
        } else {
            None
        }
    }

    /// If the current request type is [`Notification`] returns a mutable reference to it,
    /// otherwise returns `None`
    #[inline]
    pub fn notification_mut(&mut self) -> Option<&mut Notification> {
        if let Message::Notification(notify) = &mut self.msg {
            Some(notify)
        } else {
            None
        }
    }
}

impl Middlewares {
    /// Initializes a new middleware pipeline
    pub(super) fn new() -> Self {
        Self { pipeline: Vec::with_capacity(DEFAULT_MW_CAPACITY) }
    }

    /// Adds middleware function to the pipeline
    #[inline]
    pub(super) fn add(&mut self, middleware: Middleware) {
        self.pipeline.push(middleware);
    }

    /// Composes middlewares into a "Linked List" and returns head
    pub(super) fn compose(&self) -> Option<Next> {
        if self.pipeline.is_empty() {
            return None;
        }

        let request_handler = self.pipeline
            .last()
            .unwrap()
            .clone();
        
        let mut next: Next = Arc::new(move |ctx| request_handler(
            ctx, 
            Arc::new(|ctx| Box::pin(async move { Response::empty(ctx.id()) }))
        ));
        for mw in self.pipeline.iter().rev().skip(1) {
            let current_mw: Middleware = mw.clone();
            let prev_next: Next = next.clone();
            next = Arc::new(move |ctx| current_mw(ctx, prev_next.clone()));
        }
        Some(next)
    }
}
