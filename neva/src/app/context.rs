//! Server runtime context utilities

use tokio::time::timeout;
use crate::error::{Error, ErrorCode};
use crate::transport::Sender;
use crate::types::Response;
use super::{options::{McpOptions, RuntimeMcpOptions}, handler::RequestHandler};
use crate::{
    shared::RequestQueue,
    types::{RequestId, Request, root::{ListRootsRequestParams, ListRootsResult}},
    transport::TransportProtoSender
};
use std::{
    collections::HashMap,
    time::Duration,
    sync::Arc
};

type RequestHandlers = HashMap<String, RequestHandler<Response>>;

/// Represents a Server runtime
#[derive(Clone)]
pub(crate) struct ServerRuntime {
    options: RuntimeMcpOptions,
    handlers: Arc<RequestHandlers>,
    pending: RequestQueue,
    sender: TransportProtoSender,
}

/// Represents MCP Request Context
#[derive(Clone)]
pub struct Context {
    pending: RequestQueue,
    sender: TransportProtoSender,
    timeout: Duration,
}

impl ServerRuntime {
    /// Creates a new server runtime
    pub(crate) fn new(
        sender: TransportProtoSender, 
        options: McpOptions,
        handlers: RequestHandlers,
    ) -> Self {
        Self {
            pending: Default::default(),
            handlers: Arc::new(handlers),
            options: Arc::new(options),
            sender,
        }
    }
    
    /// Provides a [`RuntimeMcpOptions`]
    pub(crate) fn options(&self) ->  RuntimeMcpOptions {
        self.options.clone()
    }

    /// Provides the current connections sender
    pub(crate) fn sender(&self) ->  TransportProtoSender {
        self.sender.clone()
    }
    
    /// Provides a hash map of registered request handlers
    pub(crate) fn request_handlers(&self) ->  Arc<RequestHandlers> {
        self.handlers.clone()
    }
    
    /// Creates a new MCP request [`Context`]
    pub(crate) fn context(&self) -> Context {
        Context {
            pending: self.pending.clone(),
            sender: self.sender.clone(),
            timeout: self.options.request_timeout,
        }
    }
    
    /// Provides a "queue" of pending requests
    pub(crate) fn pending_requests(&self) -> &RequestQueue {
        &self.pending
    }
}


impl Context {
    /// Requests a list of available roots from a client
    /// 
    /// # Example
    /// ```no_run
    /// use neva::{Context, error::Error};
    /// 
    /// async fn handle_roots(mut ctx: Context) -> Result<(), Error> {
    ///     let roots = ctx.list_roots().await?;
    /// 
    ///     // do something with roots
    /// 
    /// # Ok(())
    /// }
    /// ```
    pub async fn list_roots(&mut self) -> Result<ListRootsResult, Error> {
        let id = RequestId::String(crate::types::root::commands::LIST.into());
        let req = Request::new(
            Some(id.clone()),
            crate::types::root::commands::LIST,
            Some(ListRootsRequestParams::default()));

        let receiver = self.pending.push(&id).await;
        self.sender.send(req.into()).await?;

        match timeout(self.timeout, receiver).await {
            Ok(Ok(resp)) => Ok(resp.into_result()?),
            Ok(Err(_)) => Err(Error::new(ErrorCode::InternalError, "Response channel closed")),
            Err(_) => {
                _ = self.pending.pop(&id).await;
                Err(Error::new(ErrorCode::Timeout, "Request timed out"))
            }
        }
    }
}