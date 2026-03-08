//! Fluent batch request builder for the MCP client

use super::Client;
use crate::{
    error::Error,
    shared::IntoArgs,
    types::{
        MessageEnvelope, Request, RequestId, RequestParamsMeta, Response,
        notification::Notification,
        resource::Uri,
    },
};

/// A fluent builder for constructing and sending a JSON-RPC 2.0 batch request.
///
/// Obtain via [`Client::batch`]. Add requests with the provided methods —
/// which mirror [`Client`]'s single-call API — then call [`BatchBuilder::send`].
///
/// # Example
/// ```no_run
/// use neva::client::Client;
/// use neva::error::Error;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Error> {
///     let mut client = Client::new();
///     client.connect().await?;
///
///     let responses = client
///         .batch()
///         .list_tools()
///         .call_tool("add", [("a", 1), ("b", 2)])
///         .read_resource("file:///readme.md")
///         .get_prompt("summarise", [("lang", "en")])
///         .send()
///         .await?;
///
///     println!("{responses:?}");
///     client.disconnect().await
/// }
/// ```
pub struct BatchBuilder<'a> {
    pub(super) client: &'a mut Client,
    pub(super) items: Vec<MessageEnvelope>,
}

impl std::fmt::Debug for BatchBuilder<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchBuilder")
            .field("items_len", &self.items.len())
            .finish_non_exhaustive()
    }
}

impl<'a> BatchBuilder<'a> {
    /// Returns the number of enqueued items.
    #[inline]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns `true` if no items have been added yet.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Enqueues a `tools/list` request (first page; cursor is always `None`).
    pub fn list_tools(mut self) -> Self {
        use crate::types::{ListToolsRequestParams, tool::commands};
        self.push_request(commands::LIST, Some(ListToolsRequestParams { cursor: None }));
        self
    }

    /// Enqueues a `tools/call` request.
    pub fn call_tool<Args: IntoArgs>(mut self, name: impl Into<String>, args: Args) -> Self {
        use crate::types::{CallToolRequestParams, tool::commands};
        let id = self.next_id();
        let params = CallToolRequestParams {
            name: name.into(),
            args: args.into_args(),
            meta: Some(RequestParamsMeta::new(&id)),
            #[cfg(feature = "tasks")]
            task: None,
        };
        let req = Request::new(Some(id), commands::CALL, Some(params));
        self.items.push(MessageEnvelope::Request(req));
        self
    }

    /// Enqueues a `resources/list` request (first page; cursor is always `None`).
    pub fn list_resources(mut self) -> Self {
        use crate::types::{ListResourcesRequestParams, resource::commands};
        self.push_request(commands::LIST, Some(ListResourcesRequestParams { cursor: None }));
        self
    }

    /// Enqueues a `resources/read` request.
    pub fn read_resource(mut self, uri: impl Into<Uri>) -> Self {
        use crate::types::{ReadResourceRequestParams, resource::commands};
        let id = self.next_id();
        let params = ReadResourceRequestParams {
            uri: uri.into(),
            meta: Some(RequestParamsMeta::new(&id)),
            #[cfg(feature = "server")]
            args: None,
        };
        let req = Request::new(Some(id), commands::READ, Some(params));
        self.items.push(MessageEnvelope::Request(req));
        self
    }

    /// Enqueues a `resources/templates/list` request.
    pub fn list_resource_templates(mut self) -> Self {
        use crate::types::{ListResourceTemplatesRequestParams, resource::commands};
        self.push_request(commands::TEMPLATES_LIST, Some(ListResourceTemplatesRequestParams { cursor: None }));
        self
    }

    /// Enqueues a `prompts/list` request (first page; cursor is always `None`).
    pub fn list_prompts(mut self) -> Self {
        use crate::types::{ListPromptsRequestParams, prompt::commands};
        self.push_request(commands::LIST, Some(ListPromptsRequestParams { cursor: None }));
        self
    }

    /// Enqueues a `prompts/get` request.
    pub fn get_prompt<Args: IntoArgs>(mut self, name: impl Into<String>, args: Args) -> Self {
        use crate::types::{GetPromptRequestParams, prompt::commands};
        let id = self.next_id();
        let params = GetPromptRequestParams {
            name: name.into(),
            meta: Some(RequestParamsMeta::new(&id)),
            args: args.into_args(),
        };
        let req = Request::new(Some(id), commands::GET, Some(params));
        self.items.push(MessageEnvelope::Request(req));
        self
    }

    /// Enqueues a `ping` request.
    pub fn ping(mut self) -> Self {
        self.push_request(crate::commands::PING, None::<()>);
        self
    }

    /// Enqueues a fire-and-forget notification (produces no response slot).
    ///
    /// `params` must already be a [`serde_json::Value`] (or `None`). Use
    /// [`serde_json::json!`] or [`serde_json::to_value`] to convert a typed value.
    pub fn notify(mut self, method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        let method = method.into();
        let notification = Notification::new(&method, params);
        self.items.push(MessageEnvelope::Notification(notification));
        self
    }

    /// Sends the batch and returns responses in request order.
    ///
    /// Notifications are included in the wire payload but produce no slot
    /// in the returned `Vec`.
    ///
    /// # Errors
    /// Returns [`Error`] if the client is not connected, the batch is empty,
    /// or any response channel is closed or times out.
    pub async fn send(self) -> Result<Vec<Response>, Error> {
        self.client.call_batch(self.items).await
    }

    /// Returns the next globally-unique [`RequestId`] from the client's handler,
    /// or a local sequential fallback if the client is not connected.
    /// (A disconnected client will fail on [`send`](Self::send) anyway.)
    fn next_id(&self) -> RequestId {
        self.client.generate_id().unwrap_or(RequestId::Number(self.items.len() as i64))
    }

    /// Enqueues a generic request with the given method and params.
    fn push_request<T: serde::Serialize>(&mut self, method: &str, params: Option<T>) {
        let id = self.next_id();
        let req = Request::new(Some(id), method, params);
        self.items.push(MessageEnvelope::Request(req));
    }
}
