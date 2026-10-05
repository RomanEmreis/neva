//! `resources/*`: listing a server's resources and templates, and reading them.

use super::{Client, collect_pages};
use crate::error::{Error, ErrorCode};
use crate::types::{
    ListResourceTemplatesRequestParams, ListResourceTemplatesResult, ListResourcesRequestParams,
    ListResourcesResult, ReadResourceRequestParams, ReadResourceResult, Request, RequestParamsMeta,
    Resource, Response, Uri,
    cursor::Cursor,
    resource::{SubscribeRequestParams, UnsubscribeRequestParams},
};
use std::fmt::{Debug, Formatter};

/// The `resources/*` methods of a connected server, from
/// [`Client::resources`].
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
///     let resources = client.resources();
///     for resource in resources.list_all().await? {
///         let contents = resources.read(resource.uri).await?;
///     }
///
///     client.disconnect().await
/// }
/// ```
#[derive(Clone, Copy)]
pub struct Resources<'a> {
    client: &'a Client,
}

impl Debug for Resources<'_> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resources").finish_non_exhaustive()
    }
}

impl<'a> Resources<'a> {
    #[inline]
    pub(super) fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// Requests one page of the server's resources (`resources/list`).
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
    ///     let first = client.resources().list(None).await?;
    ///     if let Some(cursor) = first.next_cursor {
    ///         let second = client.resources().list(Some(cursor)).await?;
    ///     }
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn list(&self, cursor: Option<Cursor>) -> Result<ListResourcesResult, Error> {
        let params = ListResourcesRequestParams { cursor };
        self.client
            .command(crate::types::resource::commands::LIST, Some(params))
            .await?
            .into_result()
    }

    /// Requests every page of the server's resources and returns them
    /// together.
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
    ///     let resources = client.resources().list_all().await?;
    ///     println!("{} resources", resources.len());
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn list_all(&self) -> Result<Vec<Resource>, Error> {
        let this = *self;
        collect_pages(
            crate::types::resource::commands::LIST,
            move |cursor| async move {
                let page = this.list(cursor).await?;
                Ok((page.resources, page.next_cursor))
            },
        )
        .await
    }

    /// Requests one page of the server's resource templates
    /// (`resources/templates/list`).
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
    ///     let templates = client.resources().templates(None).await?;
    ///     if let Some(cursor) = templates.next_cursor {
    ///         let more = client.resources().templates(Some(cursor)).await?;
    ///     }
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn templates(
        &self,
        cursor: Option<Cursor>,
    ) -> Result<ListResourceTemplatesResult, Error> {
        let params = ListResourceTemplatesRequestParams { cursor };
        self.client
            .command(
                crate::types::resource::commands::TEMPLATES_LIST,
                Some(params),
            )
            .await?
            .into_result()
    }

    /// Reads the contents of the resource at `uri` (`resources/read`).
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
    ///     println!("{:?}", readme.contents);
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn read(&self, uri: impl Into<Uri>) -> Result<ReadResourceResult, Error> {
        let client = self.client;
        let id = client.generate_id()?;
        let request = Request::new(
            Some(id.clone()),
            crate::types::resource::commands::READ,
            Some(ReadResourceRequestParams {
                uri: uri.into(),
                meta: Some(RequestParamsMeta::new(&id)),
                #[cfg(feature = "server")]
                args: None,
            }),
        );

        client.send_request(request).await?.into_result()
    }

    /// Subscribes to changes of the resource at `uri` (`resources/subscribe`).
    ///
    /// Legacy only in effect: MCP 2026-07-28 folds per-resource subscriptions
    /// into the `Client::listen` filter, so against a 2026-07-28 peer this
    /// fails and `listen` with a `resourceSubscriptions` entry is the way. The
    /// method stays available because the dual-mode fallback still reaches
    /// legacy peers.
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
    ///     client.resources().subscribe("file:///readme.md").await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn subscribe(&self, uri: impl Into<Uri>) -> Result<(), Error> {
        #[cfg(not(feature = "legacy-spec"))]
        if !self.client.is_legacy_peer() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "resources/subscribe is legacy-only; use listen with a resource filter",
            ));
        }
        self.ensure_subscriptions()?;

        let params = SubscribeRequestParams::from(uri);
        let resp = self
            .client
            .command(crate::types::resource::commands::SUBSCRIBE, Some(params))
            .await?;

        match resp {
            Response::Ok(_) => Ok(()),
            Response::Err(err) => Err(err.error.into()),
        }
    }

    /// Ends a subscription to changes of the resource at `uri`
    /// (`resources/unsubscribe`).
    ///
    /// Legacy only in effect; see [`Self::subscribe`]. Under MCP 2026-07-28 a
    /// subscription ends with the stream that carries it
    /// (`Subscription::cancel`).
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
    ///     client.resources().unsubscribe("file:///readme.md").await?;
    ///
    ///     client.disconnect().await
    /// }
    /// ```
    pub async fn unsubscribe(&self, uri: impl Into<Uri>) -> Result<(), Error> {
        #[cfg(not(feature = "legacy-spec"))]
        if !self.client.is_legacy_peer() {
            return Err(Error::new(
                ErrorCode::MethodNotFound,
                "resources/unsubscribe is legacy-only; cancel the subscription instead",
            ));
        }
        self.ensure_subscriptions()?;

        let params = UnsubscribeRequestParams::from(uri);
        let resp = self
            .client
            .command(crate::types::resource::commands::UNSUBSCRIBE, Some(params))
            .await?;

        match resp {
            Response::Ok(_) => Ok(()),
            Response::Err(err) => Err(err.error.into()),
        }
    }

    #[inline]
    fn ensure_subscriptions(&self) -> Result<(), Error> {
        if self.client.is_resource_subscription_supported() {
            Ok(())
        } else {
            Err(Error::new(
                ErrorCode::MethodNotFound,
                "Server does not support resource subscriptions",
            ))
        }
    }
}
