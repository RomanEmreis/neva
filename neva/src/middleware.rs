//! MCP Server middleware utilities

use std::{future::Future, sync::Arc};
use futures_util::future::BoxFuture;
use crate::{
    App, app::context::ServerRuntime, 
    types::{Message, RequestId, Request, Response, notification::Notification}
};

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

/// Turns a closure into middleware
#[inline]
pub(super) fn make_mw<F, R>(f: F) -> Middleware
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
fn make_on<F, P, R>(f: F, p: P)  -> Middleware
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
fn make_on_command<F, R>(f: F, command: &'static str)  -> Middleware
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

impl App {
    /// Registers a global middleware
    pub fn with<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_mw(middleware));
        self
    }

    /// Registers a global middleware that runs only 
    /// if the MCP server received a notification message
    pub fn with_notification<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on(
            middleware,
            |msg| msg.is_notification()));
        self
    }

    /// Registers a global middleware that runs only 
    /// if the MCP server received a request message
    pub fn with_request<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on(
            middleware,
            |msg| msg.is_request()));
        self
    }

    /// Registers a global middleware that runs only 
    /// if the MCP server received a response message
    pub fn with_response<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on(
            middleware, 
            |msg| msg.is_response()));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `tools/call` request
    pub fn with_tool<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::tool::commands::CALL));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `prompts/get` request
    pub fn with_prompt<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::prompt::commands::GET));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `resources/read` request
    pub fn with_resource<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::resource::commands::READ));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `resources/list` request
    pub fn with_list_resources<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::resource::commands::LIST));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `tools/list` request
    pub fn with_list_tool<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::tool::commands::LIST));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `prompts/list` request
    pub fn with_list_prompts<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::prompt::commands::LIST));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received an `initialize` request
    pub fn with_init<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::commands::INIT));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `tools/call` request
    pub fn for_tool<F, R>(&mut self, name: &'static str, middleware: F) -> &mut Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on(
            middleware,
            move |msg| {
                if let Message::Request(req) = msg { 
                    req.method == crate::types::tool::commands::CALL &&
                    req.params
                        .as_ref()
                        .is_some_and(|p| p.get("name")
                            .is_some_and(|n| n == name))
                } else { 
                    false
                }
            }));
        self
    }
}