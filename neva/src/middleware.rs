//! MCP Server middleware utilities

use std::{future::Future, sync::Arc};
use futures_util::future::BoxFuture;
use crate::{
    App, Context, 
    app::context::ServerRuntime, 
    types::{Message, RequestId, Request, Response, notification::Notification}
};

const DEFAULT_MW_CAPACITY: usize = 8;

pub enum MwContext {
    Message(MessageContext),
    Request(RequestContext),
    Response(ResponseContext),
    Notification(NotificationContext)
}

pub struct MessageContext {
    pub msg: Message,
    pub(super) runtime: ServerRuntime
}

pub struct RequestContext {
    pub req: Request,
    pub ctx: Context,
}

pub struct ResponseContext {
    pub resp: Response,
    pub ctx: Context,
}

pub struct NotificationContext {
    pub notification: Notification,
    pub ctx: Context,
}

pub type Next = Arc<
    dyn Fn(MwContext) -> BoxFuture<'static, Response>
    + Send 
    + Sync
>;

pub(super) type Middleware = Arc<
    dyn Fn(MwContext, Next) -> BoxFuture<'static, Response>
    + Send
    + Sync
>;

/// Turns a closure into middleware
#[inline]
fn make_mw<F, R>(f: F) -> Middleware
where
    F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
    R: Future<Output = Response> + Send + 'static,
{
    Arc::new(move |ctx: MwContext, next: Next| {
        Box::pin(f(ctx, next))
    })
}

pub(super) struct Middlewares {
    pub(super) pipeline: Vec<Middleware>
}

impl MwContext {
    /// Creates a new middleware message context
    #[inline]
    pub(super) fn msg(msg: Message, runtime: ServerRuntime) -> Self {
        Self::Message(MessageContext { msg, runtime })
    }

    /// Creates a new middleware request context
    #[inline]
    pub(super) fn req(req: Request, ctx: Context) -> Self {
        Self::Request(RequestContext { req, ctx })
    }

    /// Creates a new middleware response context
    #[inline]
    pub(super) fn resp(resp: Response, ctx: Context) -> Self {
        Self::Response(ResponseContext { resp, ctx })
    }

    /// Creates a new middleware notification context
    #[inline]
    pub(super) fn notification(notification: Notification, ctx: Context) -> Self {
        Self::Notification(NotificationContext { notification, ctx })
    }
    
    /// Returns current MCP [`Message`] ID
    #[inline]
    pub fn id(&self) -> RequestId {
        match self { 
            Self::Message(ctx) => ctx.msg.id(),
            Self::Request(ctx) => ctx.req.id.clone(),
            Self::Response(ctx) => ctx.resp.id().clone(),
            _ => RequestId::default()
        }
    }

    /// Returns current MCP session ID
    #[inline]
    pub fn session_id(&self) -> Option<&uuid::Uuid> {
        match self {
            Self::Message(ctx) => ctx.msg.session_id(),
            Self::Request(ctx) => ctx.req.session_id.as_ref(),
            Self::Response(ctx) => ctx.resp.session_id(),
            Self::Notification(ctx) => ctx.notification.session_id.as_ref()
        }
    }

    /// If the current message type is [`Request`] returns a reference to it,
    /// otherwise returns `None`
    #[inline]
    pub fn request(&self) -> Option<&Request> {
        match self {
            Self::Request(ctx) => Some(&ctx.req),
            Self::Message(ctx) => if let Message::Request(req) = &ctx.msg { 
                Some(req)
            } else { 
                None
            }, 
            _ => None
        }
    }

    /// If the current message type is [`Request`] returns a mutable reference to it,
    /// otherwise returns `None`
    #[inline]
    pub fn request_mut(&mut self) -> Option<&mut Request> {
        match self {
            Self::Request(ctx) => Some(&mut ctx.req),
            Self::Message(ctx) => if let Message::Request(req) = &mut ctx.msg {
                Some(req)
            } else {
                None
            },
            _ => None
        }
    }

    /// If the current request type is [`Response`] returns a reference to it,
    /// otherwise returns `None`
    pub fn response(&self) -> Option<&Response> {
        match self {
            Self::Response(ctx) => Some(&ctx.resp),
            Self::Message(ctx) => if let Message::Response(resp) = &ctx.msg {
                Some(resp)
            } else {
                None
            },
            _ => None
        }
    }

    /// If the current request type is [`Response`] returns a mutable reference to [`ResponseContext`],
    /// otherwise returns `None`
    pub fn response_mut(&mut self) -> Option<&mut Response> {
        match self {
            Self::Response(ctx) => Some(&mut ctx.resp),
            Self::Message(ctx) => if let Message::Response(resp) = &mut ctx.msg {
                Some(resp)
            } else {
                None
            },
            _ => None
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
            next = Arc::new(move |ctx| {
                //let current_mw = current_mw.clone();
                let prev_next = prev_next.clone();
                current_mw(ctx, prev_next)
            });
        }
        Some(next)
    }
}

impl App {
    /// Registers a middleware
    pub fn with<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.with_ref(middleware);
        self
    }

    /// Registers a middleware
    pub(super) fn with_ref<F, R>(&mut self, middleware: F) -> &mut Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_mw(middleware));
        self
    }
}