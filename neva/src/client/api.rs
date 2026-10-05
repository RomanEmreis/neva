//! A connected server's primitives, one namespace per MCP method prefix.
//!
//! [`Client::tools`] is `tools/*`, [`Client::resources`] is `resources/*`,
//! [`Client::prompts`] is `prompts/*` and `Client::tasks` (feature `tasks`) is
//! `tasks/*`:
//! `client.tools().list(None)` sends `tools/list`, `client.prompts().get(..)`
//! sends `prompts/get`.
//!
//! Each namespace is a `Copy` view over a shared borrow of the client, so any
//! number of them can be held at once, and next to [`Client::batch`].
//!
//! # Examples
//! ```no_run
//! use neva::client::Client;
//! use neva::error::Error;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Error> {
//!     let mut client = Client::new();
//!     client.connect().await?;
//!
//!     let (tools, prompts) = (client.tools(), client.prompts());
//!     let listed = tools.list_all().await?;
//!     let result = tools.call("add", [("a", 1), ("b", 2)]).await?;
//!     let prompt = prompts.get("summarise", [("lang", "en")]).await?;
//!
//!     client.disconnect().await
//! }
//! ```

use super::Client;
use crate::error::{Error, ErrorCode};
use crate::types::cursor::Cursor;
use std::future::Future;

mod prompts;
mod resources;
#[cfg(feature = "tasks")]
mod tasks;
mod tools;

pub use prompts::Prompts;
pub use resources::Resources;
#[cfg(feature = "tasks")]
pub use tasks::Tasks;
pub use tools::Tools;

/// How many pages a traversal walks before giving up: the `list_all` helpers
/// and the `HeaderMismatch` recovery.
///
/// A traversal ends on its own at a page without a `nextCursor`; this is the
/// bound for a server that never stops handing them out, which would otherwise
/// keep one call walking forever with nothing above it able to see.
pub(super) const MAX_LIST_PAGES: usize = 64;

/// Walks a paginated listing from its first page, `fetch` answering one page
/// and the cursor of the next.
///
/// A server still paging at [`MAX_LIST_PAGES`] is an error rather than the
/// pages so far: a listing cut short would look complete to the caller.
async fn collect_pages<T, F, Fut>(method: &str, mut fetch: F) -> Result<Vec<T>, Error>
where
    F: FnMut(Option<Cursor>) -> Fut,
    Fut: Future<Output = Result<(Vec<T>, Option<Cursor>), Error>>,
{
    let mut items = Vec::new();
    let mut cursor = None;
    for _ in 0..MAX_LIST_PAGES {
        let (page, next) = fetch(cursor).await?;
        items.extend(page);
        match next {
            Some(next) => cursor = Some(next),
            None => return Ok(items),
        }
    }

    Err(Error::new(
        ErrorCode::InternalError,
        format!("`{method}` was still paging after {MAX_LIST_PAGES} pages"),
    ))
}

impl Client {
    /// The server's tools: `tools/list` and `tools/call`.
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
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[inline]
    pub fn tools(&self) -> Tools<'_> {
        Tools::new(self)
    }

    /// The server's resources: `resources/list`, `resources/templates/list`
    /// and `resources/read`.
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
    ///     let readme = client.resources().read("file:///readme.md").await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[inline]
    pub fn resources(&self) -> Resources<'_> {
        Resources::new(self)
    }

    /// The server's prompts: `prompts/list` and `prompts/get`.
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
    ///     let prompt = client.prompts().get("summarise", [("lang", "en")]).await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[inline]
    pub fn prompts(&self) -> Prompts<'_> {
        Prompts::new(self)
    }

    /// The server's tasks: `tasks/get`, `tasks/cancel` and the rest of the
    /// tasks methods this protocol generation has.
    ///
    /// A task is started by a task-augmented call, through
    /// [`Tools::as_task`].
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
    ///     let task = client.tasks().get("task-1").await?;
    ///     println!("{:?}", task.status);
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    #[cfg(feature = "tasks")]
    #[inline]
    pub fn tasks(&self) -> Tasks<'_> {
        Tasks::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_page_is_collected_in_order() {
        let pages = collect_pages("things/list", |cursor: Option<Cursor>| async move {
            let page = cursor.map_or(0, |Cursor(page)| page);
            let next = (page < 2).then_some(Cursor(page + 1));
            Ok((vec![page * 10, page * 10 + 1], next))
        })
        .await
        .expect("three pages");

        assert_eq!(pages, [0, 1, 10, 11, 20, 21]);
    }

    /// A server that never stops handing out cursors would keep the walk going
    /// forever; it ends at the cap, as an error rather than a listing that
    /// looks complete.
    #[tokio::test]
    async fn a_listing_that_never_ends_is_an_error() {
        let mut fetched = 0;
        let result = collect_pages("things/list", |_: Option<Cursor>| {
            fetched += 1;
            async { Ok((vec![()], Some(Cursor(0)))) }
        })
        .await;

        let err = result.expect_err("the walk must stop");
        assert!(err.to_string().contains("things/list"), "{err}");
        assert_eq!(fetched, MAX_LIST_PAGES);
    }
}
