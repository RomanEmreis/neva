//! The tools of a connected MCP server, handed to a model.

use super::convert;
use super::offer::Offer;
use crate::Client;
use crate::error::Error;
use crate::types::{CallToolRequestParams, CallToolResponse, Tool};
use ::svir::{ToolCall, ToolResult, Toolbox};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

/// The tools of a connected MCP server as a [`Toolbox`]: `tools/list` gives
/// the descriptors a model is offered, and each call the model makes is
/// forwarded as a `tools/call`.
///
/// The descriptors are a snapshot -- [`Toolbox::tools`] is synchronous, and a
/// listing is paged over the network -- taken by [`Self::load`] and renewed by
/// [`Self::refresh`], for example when the server sends
/// `notifications/tools/list_changed`. Only the tools in the snapshot can be
/// called: a model naming another one is told there is no such tool.
///
/// Which tools a model may use is the caller's policy: [`Self::filter`]
/// narrows them, and [`Self::rename`] and [`Self::with_prefix`] choose the names
/// they are offered under, keeping the tools of several servers apart in one
/// request. A tool that can only be called as a task is not offered, nor,
/// with the `apps` feature, one MCP Apps hides from the model.
///
/// Calls go over the shared [`Client`], so [`Toolbox::call_all`] runs them
/// concurrently, answering in order.
///
/// # Examples
/// ```no_run
/// use neva::{Client, error::Error, svir::RemoteTools};
///
/// #[tokio::main]
/// async fn main() -> Result<(), Error> {
///     let mut client = Client::new();
///     client.connect().await?;
///
///     let tools = RemoteTools::new(client)
///         .with_prefix("docs_")
///         .filter(|tool| tool.name != "delete_everything")
///         .load()
///         .await?;
///
///     // `svir::Request::new(model).tools(&tools)` offers them, and
///     // `tools.call_all(&completion.tool_calls)` answers the model.
///     Ok(())
/// }
/// ```
pub struct RemoteTools {
    client: Arc<Client>,
    offer: Offer,
}

impl Debug for RemoteTools {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteTools")
            .field("offer", &self.offer)
            .finish_non_exhaustive()
    }
}

impl RemoteTools {
    /// The tools of the server `client` is connected to, not loaded yet:
    /// configure them, then [`Self::load`].
    ///
    /// Takes the client itself, or an `Arc` of one that is shared with the
    /// rest of the program; [`Self::client`] reaches it either way.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{Client, error::Error, svir::RemoteTools};
    ///
    /// # async fn run(client: Client) -> Result<(), Error> {
    /// let tools = RemoteTools::new(client).load().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(client: impl Into<Arc<Client>>) -> Self {
        Self {
            client: client.into(),
            offer: Offer::default(),
        }
    }

    /// The client the tools are called through, for the rest of what the
    /// server offers.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{error::Error, svir::RemoteTools};
    ///
    /// # async fn run(tools: RemoteTools) -> Result<(), Error> {
    /// let prompts = tools.client().prompts().list_all().await?;
    /// # Ok(())
    /// # }
    /// ```
    #[inline]
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Offers only the tools `filter` keeps.
    ///
    /// Filters add up: a tool is offered only if every one of them keeps it.
    /// Each sees the tool as the server listed it, under the server's name,
    /// whatever renames come before or after it.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{Client, error::Error, svir::RemoteTools};
    ///
    /// # async fn run(client: Client) -> Result<(), Error> {
    /// let tools = RemoteTools::new(client)
    ///     .filter(|tool| !tool.name.starts_with("admin_"))
    ///     .filter(|tool| tool.name != "delete_everything")
    ///     .load()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn filter<F>(mut self, filter: F) -> Self
    where
        F: Fn(&Tool) -> bool + Send + Sync + 'static,
    {
        self.offer.filter(Box::new(filter));
        self
    }

    /// Offers each tool under the name `rename` makes of it. A call is
    /// forwarded under the name the server knows.
    ///
    /// Renames add up, in order: each one is given the name the previous ones
    /// made. A name is never changed silently -- one a model API cannot carry
    /// fails the snapshot (see [`Self::refresh`]) -- so this is also the way to
    /// offer a tool whose server name has a `.` in it.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{Client, error::Error, svir::RemoteTools};
    ///
    /// # async fn run(client: Client) -> Result<(), Error> {
    /// // The server's `files.read` is offered as `gh_files_read`.
    /// let tools = RemoteTools::new(client)
    ///     .rename(|name| name.replace('.', "_"))
    ///     .with_prefix("gh_")
    ///     .load()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn rename<F>(mut self, rename: F) -> Self
    where
        F: Fn(&str) -> String + Send + Sync + 'static,
    {
        self.offer.rename(Box::new(rename));
        self
    }

    /// Offers each tool as `prefix` followed by its name, so tools of two
    /// servers do not collide in one request: a [`Self::rename`] that
    /// prepends `prefix`.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{Client, error::Error, svir::RemoteTools};
    ///
    /// # async fn run(client: Client) -> Result<(), Error> {
    /// // The server's `search` is offered as `docs_search`.
    /// let tools = RemoteTools::new(client).with_prefix("docs_").load().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_prefix(self, prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        self.rename(move |name| format!("{prefix}{name}"))
    }

    /// Takes the first snapshot of the server's tools.
    ///
    /// Fails as [`Self::refresh`] does.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{Client, error::Error, svir::RemoteTools};
    ///
    /// # async fn run(client: Client) -> Result<(), Error> {
    /// let tools = RemoteTools::new(client).load().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn load(self) -> Result<Self, Error> {
        self.refresh().await?;
        Ok(self)
    }

    /// Replaces the snapshot with the server's current tools.
    ///
    /// Fails if the listing fails, or if a tool that would be offered has a
    /// name a model API cannot carry (`[a-zA-Z0-9_-]{1,64}`, after the
    /// renames), or shares its name with another. Such a tool is refused
    /// rather than renamed behind the caller's back: [`Self::rename`] can
    /// give it a name, and [`Self::filter`] can leave it out. A failed refresh
    /// keeps the previous snapshot.
    ///
    /// # Examples
    /// ```no_run
    /// use neva::{error::Error, svir::RemoteTools};
    ///
    /// # async fn run(tools: RemoteTools) -> Result<(), Error> {
    /// // On `notifications/tools/list_changed`:
    /// tools.refresh().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn refresh(&self) -> Result<(), Error> {
        let listed = self.client.tools().list_all().await?;
        self.offer.replace(listed)
    }
}

impl Toolbox for RemoteTools {
    fn tools(&self) -> Vec<::svir::Tool> {
        self.offer.tools()
    }

    async fn call(&self, call: &ToolCall) -> ToolResult {
        let Some(name) = self.offer.server_name(&call.name) else {
            return ToolResult::error(&call.id, format!("There is no tool named `{}`", call.name));
        };

        let args = match convert::arguments(call) {
            Ok(args) => args,
            Err(err) => return ToolResult::error(&call.id, err),
        };

        let params = CallToolRequestParams::new(name).with_args(args);
        let outcome = self
            .client
            .tools()
            .call_raw(params)
            .await
            .and_then(|response| response.into_result::<CallToolResponse>());

        convert::result(&call.id, outcome)
    }

    /// Answers the calls concurrently, each a `tools/call` of its own over the
    /// shared client, in the order they were made.
    fn call_all(&self, calls: &[ToolCall]) -> impl Future<Output = Vec<ToolResult>> + Send
    where
        Self: Sync,
    {
        futures_util::future::join_all(calls.iter().map(|call| self.call(call)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toolbox is handed to whatever drives the model, often a spawned task.
    #[test]
    fn a_toolbox_can_be_shared_across_tasks() {
        fn shared<T: Send + Sync + 'static>() {}
        shared::<RemoteTools>();
    }
}
