//! `tools/*`: listing a server's tools and calling them.

use super::{Client, collect_pages};
use crate::error::Error;
use crate::shared::IntoArgs;
use crate::types::{
    CallToolRequestParams, CallToolResponse, ListToolsResult, Request, Response, Tool,
    cursor::Cursor,
};
use std::fmt::{Debug, Formatter};

#[cfg(feature = "tasks")]
use crate::client::TaskBuilder;

/// The `tools/*` methods of a connected server, from [`Client::tools`].
///
/// # Examples
/// ```no_run
/// use neva::client::Client;
/// use neva::error::Error;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Error> {
///     let mut client = Client::new();
///     client.connect().await?;
///
///     let tools = client.tools();
///     for tool in tools.list_all().await? {
///         println!("{}", tool.name);
///     }
///     let result = tools.call("add", [("a", 1), ("b", 2)]).await?;
///
///     client.disconnect().await
/// }
/// ```
#[derive(Clone, Copy)]
pub struct Tools<'a> {
    client: &'a Client,
}

impl Debug for Tools<'_> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tools").finish_non_exhaustive()
    }
}

impl<'a> Tools<'a> {
    #[inline]
    pub(super) fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// Requests one page of the server's tools (`tools/list`).
    ///
    /// `None` asks for the first page; a page's `next_cursor` asks for the one
    /// after it. [`Self::list_all`] walks every page.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     let first = client.tools().list(None).await?;
    ///     if let Some(cursor) = first.next_cursor {
    ///         let second = client.tools().list(Some(cursor)).await?;
    ///     }
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn list(&self, cursor: Option<Cursor>) -> Result<ListToolsResult, Error> {
        self.client
            .list_tools_inner(
                cursor,
                #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
                None,
            )
            .await
    }

    /// Requests every page of the server's tools and returns them together.
    ///
    /// Fails rather than return a partial list if the server is still paging
    /// after 64 pages.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     let tools = client.tools().list_all().await?;
    ///     println!("{} tools", tools.len());
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn list_all(&self) -> Result<Vec<Tool>, Error> {
        let this = *self;
        collect_pages(
            crate::types::tool::commands::LIST,
            move |cursor| async move {
                let page = this.list(cursor).await?;
                Ok((page.tools, page.next_cursor))
            },
        )
        .await
    }

    /// Calls the tool `name` with `args` (`tools/call`).
    ///
    /// # Examples
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     let result = client.tools().call("add", [("a", 1), ("b", 2)]).await?;
    ///     println!("{:?}", result.content);
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn call<N, Args>(&self, name: N, args: Args) -> Result<CallToolResponse, Error>
    where
        N: Into<String>,
        Args: IntoArgs,
    {
        let params = CallToolRequestParams {
            name: name.into(),
            meta: None,
            args: args.into_args(),
            #[cfg(feature = "tasks")]
            task: None,
        };

        self.call_raw(params).await?.into_result()
    }

    /// Calls a tool with fully formed `params` and returns the raw JSON-RPC
    /// response, error responses included.
    ///
    /// The `_meta` the params carry goes out as given, a `traceparent` say,
    /// but for the progress token: that one is the client's, since progress
    /// notifications find their call by it.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    /// use neva::types::CallToolRequestParams;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     let params = CallToolRequestParams::new("add").with_args([("a", 1), ("b", 2)]);
    ///     let response = client.tools().call_raw(params).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn call_raw(&self, mut params: CallToolRequestParams) -> Result<Response, Error> {
        let client = self.client;
        let id = client.generate_id()?;
        params.track_progress(&id);

        // Serialized from a borrow: the params stay at hand for the SEP-2243
        // retry, which the call cannot be rebuilt for from its own answer,
        // without a copy of the arguments on every call.
        let request = Request::new(Some(id), crate::types::tool::commands::CALL, Some(&params));

        #[cfg(all(feature = "http-client", not(feature = "legacy-spec")))]
        {
            let resp = client.send_request(request).await?;
            client.retry_after_header_mismatch(resp, params).await
        }
        #[cfg(not(all(feature = "http-client", not(feature = "legacy-spec"))))]
        client.send_request(request).await
    }

    /// Starts a task-augmented call: configure the task on the returned
    /// [`TaskBuilder`], then [`TaskBuilder::call`] the tool.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::client::Client;
    /// use neva::error::Error;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Error> {
    ///     let mut client = Client::new();
    ///     client.connect().await?;
    ///
    ///     let result = client
    ///         .tools()
    ///         .as_task()
    ///         .with_ttl(5000)
    ///         .call("echo", [("message", "Hello MCP!")])
    ///         .await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[cfg(feature = "tasks")]
    #[inline]
    pub fn as_task(&self) -> TaskBuilder<'a> {
        TaskBuilder {
            client: self.client,
            metadata: crate::types::TaskMetadata::default(),
        }
    }
}
