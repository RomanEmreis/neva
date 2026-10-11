//! MCP Server middleware utilities

use crate::shared::BoxFuture;
use crate::{
    app::context::ServerRuntime,
    auth::Claims,
    types::{Message, Request, RequestId, Response, notification::Notification},
};
use std::fmt::Debug;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[cfg(feature = "di")]
use {crate::error::Error, volga_di::Container};

pub(super) mod make_fn;
pub mod wrap;

const DEFAULT_MW_CAPACITY: usize = 8;

/// Current middleware operation context.
pub struct MwContext {
    /// Current JSON-RPC message
    pub msg: Message,

    /// Server runtime reference
    pub(super) runtime: ServerRuntime,

    /// Who sends a request's reply: see [`ReplyClaim`].
    pub(super) reply: Arc<AtomicBool>,

    /// Dependency injection container scope.
    #[cfg(feature = "di")]
    pub(super) scope: Container,
}

impl Debug for MwContext {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MwContext").field("msg", &self.msg).finish()
    }
}

/// The right to send a request's reply, so that exactly one is sent.
///
/// The dispatcher at the end of the pipeline takes it when the request reaches
/// it, and holds it while the handler runs and the reply goes out. Dropped
/// before [`Self::keep`] -- its future cancelled by a middleware that stopped
/// waiting for `next`, a timeout around it say -- it is given back, and the
/// answer that middleware returned is sent instead. A send cut short sent
/// nothing: both transports queue on a bounded channel, whose send is
/// cancel-safe.
///
/// The pipeline's runner takes what is left once the pipeline returns. A
/// middleware that let the request through without waiting for it, and
/// answered itself, gets its answer out first; the dispatcher then runs the
/// handler but finds the reply taken.
pub(crate) struct ReplyClaim(Option<Arc<AtomicBool>>);

impl ReplyClaim {
    /// Takes the right to send the reply, unless it is taken already.
    #[inline]
    pub(crate) fn take(reply: Arc<AtomicBool>) -> Option<Self> {
        reply
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then(|| Self(Some(reply)))
    }

    /// Keeps the right for good: the reply is out.
    #[inline]
    pub(crate) fn keep(mut self) {
        self.0 = None;
    }
}

impl Drop for ReplyClaim {
    #[inline]
    fn drop(&mut self) {
        if let Some(reply) = self.0.take() {
            reply.store(false, Ordering::Release);
        }
    }
}

/// A reference to the next middleware in the chain.
///
/// # Answering a request
///
/// A middleware that calls `next` hands the request on, and the reply the
/// caller gets is the one the request's handler produced: it is sent once the
/// request reaches the end of the pipeline, so a different [`Response`]
/// returned after `next` is not sent.
///
/// A middleware that returns without calling `next` answers the request
/// itself: what it returns is sent to the caller, over any transport and
/// inside a batch alike. So does one that stops waiting for `next` before the
/// reply is out, a timeout around it say. Build the answer with the request's
/// id, from [`MwContext::id`]. A notification gets no reply either way.
///
/// # Examples
///
/// ```no_run
/// use neva::prelude::*;
///
/// let app = App::new().wrap_tools(|ctx: MwContext, next: Next| async move {
///     if ctx.request().is_some_and(|req| req.params.is_none()) {
///         let err = Error::new(ErrorCode::InvalidParams, "a tool call needs params");
///         return Response::error(ctx.id(), err);
///     }
///     next(ctx).await
/// });
/// ```
pub type Next = Arc<dyn Fn(MwContext) -> BoxFuture<'static, Response> + Send + Sync>;

/// Middleware function wrapper
pub(super) type Middleware =
    Arc<dyn Fn(MwContext, Next) -> BoxFuture<'static, Response> + Send + Sync>;

/// MCP middleware pipeline.
#[derive(Clone)]
pub(super) struct Middlewares {
    pub(super) pipeline: Vec<Middleware>,
}

impl MwContext {
    /// Creates a new middleware message context
    #[inline]
    pub(super) fn msg(msg: Message, runtime: ServerRuntime) -> Self {
        #[cfg(feature = "di")]
        let scope = runtime.container.create_scope();
        Self {
            msg,
            runtime,
            reply: Arc::default(),
            #[cfg(feature = "di")]
            scope,
        }
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

    /// The claims of the caller's access token, when the current message is
    /// a [`Request`] from an authenticated caller.
    ///
    /// The same claims a handler reads with `Context::claims`, and `None` in
    /// the same cases: a server without bearer auth, or one built without an
    /// HTTP transport.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use neva::prelude::*;
    ///
    /// let app = App::new().wrap_tools(|ctx, next| async move {
    ///     let client = ctx.claims().and_then(|c| c.client_id()).unwrap_or("-");
    ///     eprintln!("tool call by client {client}");
    ///     next(ctx).await
    /// });
    /// ```
    #[inline]
    pub fn claims(&self) -> Option<&dyn Claims> {
        #[cfg(feature = "http-server")]
        {
            self.request()?.claims.as_deref()
        }
        #[cfg(not(feature = "http-server"))]
        {
            None
        }
    }

    /// Resolves a service and returns a cloned instance.
    /// `T` must implement `Clone` otherwise
    /// use resolve_shared method that returns a shared pointer.
    #[inline]
    #[cfg(feature = "di")]
    pub fn resolve<T: Send + Sync + Clone + 'static>(&self) -> Result<T, Error> {
        self.scope.resolve::<T>().map_err(Into::into)
    }

    /// Resolves a service and returns a shared pointer
    #[inline]
    #[cfg(feature = "di")]
    pub fn resolve_shared<T: Send + Sync + 'static>(&self) -> Result<Arc<T>, Error> {
        self.scope.resolve_shared::<T>().map_err(Into::into)
    }
}

impl Middlewares {
    /// Initializes a new middleware pipeline
    pub(super) fn new() -> Self {
        Self {
            pipeline: Vec::with_capacity(DEFAULT_MW_CAPACITY),
        }
    }

    /// Adds middleware function to the pipeline
    #[inline]
    pub(super) fn add(&mut self, middleware: Middleware) {
        self.pipeline.push(middleware);
    }

    /// Adds a middleware at the front of the pipeline, making it the outermost
    /// layer so its wrapping (e.g. the request tracing span) covers every
    /// already-registered user middleware.
    #[inline]
    #[cfg(feature = "tracing")]
    pub(super) fn add_front(&mut self, middleware: Middleware) {
        self.pipeline.insert(0, middleware);
    }

    /// Composes middlewares into a "Linked List" and returns head
    pub(super) fn compose(&self) -> Option<Next> {
        if self.pipeline.is_empty() {
            return None;
        }

        let request_handler = self.pipeline.last().unwrap().clone();
        let mut next: Next = {
            let dummy: Next = Arc::new(|ctx| Box::pin(async move { Response::empty(ctx.id()) }));
            Arc::new(move |ctx| request_handler(ctx, dummy.clone()))
        };

        for mw in self.pipeline.iter().rev().skip(1) {
            let current_mw: Middleware = mw.clone();
            let prev_next: Next = next.clone();
            next = Arc::new(move |ctx| current_mw(ctx, prev_next.clone()));
        }
        Some(next)
    }
}
