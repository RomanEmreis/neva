//! `prompts/*`: listing a server's prompts and getting them.

use super::{Client, collect_pages};
use crate::error::Error;
use crate::shared::IntoArgs;
use crate::types::{
    GetPromptRequestParams, GetPromptResult, ListPromptsRequestParams, ListPromptsResult, Prompt,
    Request, RequestParamsMeta, cursor::Cursor,
};
use std::fmt::{Debug, Formatter};

/// The `prompts/*` methods of a connected server, from [`Client::prompts`].
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
///     let prompts = client.prompts();
///     for prompt in prompts.list_all().await? {
///         println!("{}", prompt.name);
///     }
///     let prompt = prompts.get("summarise", [("lang", "en")]).await?;
///
///     client.disconnect().await
/// }
/// ```
#[derive(Clone, Copy)]
pub struct Prompts<'a> {
    client: &'a Client,
}

impl Debug for Prompts<'_> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prompts").finish_non_exhaustive()
    }
}

impl<'a> Prompts<'a> {
    #[inline]
    pub(super) fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// Requests one page of the server's prompts (`prompts/list`).
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
    ///     let first = client.prompts().list(None).await?;
    ///     if let Some(cursor) = first.next_cursor {
    ///         let second = client.prompts().list(Some(cursor)).await?;
    ///     }
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn list(&self, cursor: Option<Cursor>) -> Result<ListPromptsResult, Error> {
        let params = ListPromptsRequestParams { cursor };
        self.client
            .command(crate::types::prompt::commands::LIST, Some(params))
            .await?
            .into_result()
    }

    /// Requests every page of the server's prompts and returns them together.
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
    ///     let prompts = client.prompts().list_all().await?;
    ///     println!("{} prompts", prompts.len());
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn list_all(&self) -> Result<Vec<Prompt>, Error> {
        let this = *self;
        collect_pages(
            crate::types::prompt::commands::LIST,
            move |cursor| async move {
                let page = this.list(cursor).await?;
                Ok((page.prompts, page.next_cursor))
            },
        )
        .await
    }

    /// Gets the prompt `name`, rendered with `args` (`prompts/get`).
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
    ///     println!("{:?}", prompt.messages);
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn get<N, Args>(&self, name: N, args: Args) -> Result<GetPromptResult, Error>
    where
        N: Into<String>,
        Args: IntoArgs,
    {
        let client = self.client;
        let id = client.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            crate::types::prompt::commands::GET,
            Some(GetPromptRequestParams {
                name: name.into(),
                meta: Some(RequestParamsMeta::new(&id)),
                args: args.into_args(),
            }),
        );

        client.send_request(request).await?.into_result()
    }
}
