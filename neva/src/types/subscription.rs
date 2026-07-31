//! Long-lived notification subscriptions (MCP 2026-07-28).
//!
//! `subscriptions/listen` opens a stream the server keeps open, delivering only
//! the notification categories the client opted in to. It replaces both the
//! legacy HTTP `GET` stream and the `resources/subscribe` /
//! `resources/unsubscribe` RPC pair -- per-resource subscriptions are now the
//! [`SubscriptionFilter::resource_subscriptions`] field of the listen filter.
//!
//! See the [specification](https://modelcontextprotocol.io/specification/2026-07-28/basic/patterns/subscriptions)
//! for details.

use crate::types::{RequestId, RequestParamsMeta, Uri};
use serde::{Deserialize, Serialize};

#[cfg(feature = "server")]
use crate::app::handler::{FromHandlerParams, HandlerParams};
#[cfg(feature = "server")]
use crate::error::Error;
#[cfg(feature = "server")]
use crate::types::{IntoResponse, Request, Response, request::FromRequest};

/// `_meta` key that tags every message belonging to a subscription with the
/// id of the `subscriptions/listen` request that opened it.
pub const SUBSCRIPTION_ID_KEY: &str = "io.modelcontextprotocol/subscriptionId";

/// List of commands for subscriptions
pub mod commands {
    /// Command name that opens a long-lived notification subscription.
    pub const LISTEN: &str = "subscriptions/listen";

    /// Notification name that reports the accepted subset of a subscription
    /// filter. Always the first message on a subscription.
    pub const ACKNOWLEDGED: &str = "notifications/subscriptions/acknowledged";
}

#[inline]
fn is_false(flag: &bool) -> bool {
    !*flag
}

/// Notification categories a client opts in to on a `subscriptions/listen`
/// stream.
///
/// Every category is opt-in: a server **MUST NOT** deliver a notification type
/// the client did not request. An omitted field is exactly "not subscribed",
/// which is why unset categories are skipped on the wire rather than sent as
/// `false`.
///
/// # Examples
/// ```
/// use neva::types::SubscriptionFilter;
///
/// let filter = SubscriptionFilter::new()
///     .with_tools_changed()
///     .with_resource("file:///project/config.json");
///
/// assert!(filter.tools_list_changed);
/// assert!(!filter.prompts_list_changed);
/// ```
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionFilter {
    /// Receive `notifications/tools/list_changed` when the tool list changes.
    #[serde(default, skip_serializing_if = "is_false")]
    pub tools_list_changed: bool,

    /// Receive `notifications/prompts/list_changed` when the prompt list changes.
    #[serde(default, skip_serializing_if = "is_false")]
    pub prompts_list_changed: bool,

    /// Receive `notifications/resources/list_changed` when the resource list changes.
    #[serde(default, skip_serializing_if = "is_false")]
    pub resources_list_changed: bool,

    /// Receive `notifications/resources/updated` for these resource URIs.
    ///
    /// This is where the removed `resources/subscribe` RPC went: a per-resource
    /// subscription is a URI in this set, scoped to the lifetime of the stream.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_subscriptions: Vec<Uri>,
}

impl SubscriptionFilter {
    /// Creates an empty filter that opts in to nothing.
    ///
    /// # Examples
    /// ```
    /// use neva::types::SubscriptionFilter;
    ///
    /// assert!(SubscriptionFilter::new().is_empty());
    /// ```
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opts in to `notifications/tools/list_changed`.
    ///
    /// # Examples
    /// ```
    /// use neva::types::SubscriptionFilter;
    ///
    /// let filter = SubscriptionFilter::new().with_tools_changed();
    /// assert!(filter.tools_list_changed);
    /// ```
    #[inline]
    pub fn with_tools_changed(mut self) -> Self {
        self.tools_list_changed = true;
        self
    }

    /// Opts in to `notifications/prompts/list_changed`.
    ///
    /// # Examples
    /// ```
    /// use neva::types::SubscriptionFilter;
    ///
    /// let filter = SubscriptionFilter::new().with_prompts_changed();
    /// assert!(filter.prompts_list_changed);
    /// ```
    #[inline]
    pub fn with_prompts_changed(mut self) -> Self {
        self.prompts_list_changed = true;
        self
    }

    /// Opts in to `notifications/resources/list_changed`.
    ///
    /// # Examples
    /// ```
    /// use neva::types::SubscriptionFilter;
    ///
    /// let filter = SubscriptionFilter::new().with_resources_changed();
    /// assert!(filter.resources_list_changed);
    /// ```
    #[inline]
    pub fn with_resources_changed(mut self) -> Self {
        self.resources_list_changed = true;
        self
    }

    /// Opts in to `notifications/resources/updated` for a single resource.
    ///
    /// # Examples
    /// ```
    /// use neva::types::SubscriptionFilter;
    ///
    /// let filter = SubscriptionFilter::new().with_resource("res://config");
    /// assert_eq!(filter.resource_subscriptions.len(), 1);
    /// ```
    #[inline]
    pub fn with_resource(mut self, uri: impl Into<Uri>) -> Self {
        let uri = uri.into();
        if !self.resource_subscriptions.contains(&uri) {
            self.resource_subscriptions.push(uri);
        }
        self
    }

    /// Opts in to `notifications/resources/updated` for every supplied resource.
    ///
    /// # Examples
    /// ```
    /// use neva::types::SubscriptionFilter;
    ///
    /// let filter = SubscriptionFilter::new()
    ///     .with_resources(["res://a", "res://b"]);
    ///
    /// assert_eq!(filter.resource_subscriptions.len(), 2);
    /// ```
    #[inline]
    pub fn with_resources<U: Into<Uri>>(mut self, uris: impl IntoIterator<Item = U>) -> Self {
        for uri in uris {
            self = self.with_resource(uri);
        }
        self
    }

    /// Returns `true` when nothing at all is subscribed.
    ///
    /// # Examples
    /// ```
    /// use neva::types::SubscriptionFilter;
    ///
    /// assert!(SubscriptionFilter::new().is_empty());
    /// assert!(!SubscriptionFilter::new().with_tools_changed().is_empty());
    /// ```
    #[inline]
    pub fn is_empty(&self) -> bool {
        !self.tools_list_changed
            && !self.prompts_list_changed
            && !self.resources_list_changed
            && self.resource_subscriptions.is_empty()
    }

    /// Returns the subset present in both filters.
    ///
    /// # Examples
    /// ```
    /// use neva::types::SubscriptionFilter;
    ///
    /// let requested = SubscriptionFilter::new()
    ///     .with_tools_changed()
    ///     .with_prompts_changed();
    /// let offered = SubscriptionFilter::new().with_tools_changed();
    ///
    /// let accepted = requested.intersection(&offered);
    ///
    /// assert!(accepted.tools_list_changed);
    /// assert!(!accepted.prompts_list_changed);
    /// ```
    pub fn intersection(&self, other: &Self) -> Self {
        Self {
            tools_list_changed: self.tools_list_changed && other.tools_list_changed,
            prompts_list_changed: self.prompts_list_changed && other.prompts_list_changed,
            resources_list_changed: self.resources_list_changed && other.resources_list_changed,
            resource_subscriptions: self
                .resource_subscriptions
                .iter()
                .filter(|uri| other.resource_subscriptions.contains(uri))
                .cloned()
                .collect(),
        }
    }

    /// Returns whether this filter admits only what `other` requested.
    ///
    /// This is the client-side check behind "the server **MUST NOT** send a
    /// notification type the client has not explicitly requested": an
    /// acknowledgment that is not a subset of the request is a protocol
    /// violation.
    ///
    /// # Examples
    /// ```
    /// use neva::types::SubscriptionFilter;
    ///
    /// let requested = SubscriptionFilter::new().with_tools_changed();
    /// let acknowledged = SubscriptionFilter::new().with_tools_changed();
    ///
    /// assert!(acknowledged.is_subset_of(&requested));
    /// assert!(!requested.with_prompts_changed().is_subset_of(&acknowledged));
    /// ```
    pub fn is_subset_of(&self, other: &Self) -> bool {
        (!self.tools_list_changed || other.tools_list_changed)
            && (!self.prompts_list_changed || other.prompts_list_changed)
            && (!self.resources_list_changed || other.resources_list_changed)
            && self
                .resource_subscriptions
                .iter()
                .all(|uri| other.resource_subscriptions.contains(uri))
    }

    /// Narrows this filter to what `capabilities` actually advertise.
    ///
    /// A server answers a `subscriptions/listen` with the subset it agrees to
    /// honor; categories it never announced are dropped rather than refused.
    ///
    /// # Examples
    /// ```
    /// use neva::types::{ServerCapabilities, SubscriptionFilter, ToolsCapability};
    ///
    /// let caps = ServerCapabilities {
    ///     tools: Some(ToolsCapability { list_changed: true }),
    ///     ..Default::default()
    /// };
    ///
    /// let accepted = SubscriptionFilter::new()
    ///     .with_tools_changed()
    ///     .with_prompts_changed()
    ///     .supported_by(&caps);
    ///
    /// assert!(accepted.tools_list_changed);
    /// assert!(!accepted.prompts_list_changed);
    /// ```
    pub fn supported_by(&self, capabilities: &crate::types::ServerCapabilities) -> Self {
        let resources_subscribe = capabilities
            .resources
            .as_ref()
            .is_some_and(|res| res.subscribe);

        Self {
            tools_list_changed: self.tools_list_changed
                && capabilities
                    .tools
                    .as_ref()
                    .is_some_and(|tools| tools.list_changed),
            prompts_list_changed: self.prompts_list_changed
                && capabilities
                    .prompts
                    .as_ref()
                    .is_some_and(|prompts| prompts.list_changed),
            resources_list_changed: self.resources_list_changed
                && capabilities
                    .resources
                    .as_ref()
                    .is_some_and(|res| res.list_changed),
            resource_subscriptions: if resources_subscribe {
                self.resource_subscriptions.clone()
            } else {
                Vec::new()
            },
        }
    }

    /// Returns whether a notification belongs on a stream carrying this filter.
    ///
    /// `uri` is the updated resource for `notifications/resources/updated` and
    /// `None` for every other method.
    ///
    /// # Examples
    /// ```
    /// use neva::types::SubscriptionFilter;
    ///
    /// let filter = SubscriptionFilter::new().with_tools_changed();
    ///
    /// assert!(filter.matches("notifications/tools/list_changed", None));
    /// assert!(!filter.matches("notifications/prompts/list_changed", None));
    /// ```
    pub fn matches(&self, method: &str, uri: Option<&Uri>) -> bool {
        use crate::types::{prompt, resource, tool};

        match method {
            tool::commands::LIST_CHANGED => self.tools_list_changed,
            prompt::commands::LIST_CHANGED => self.prompts_list_changed,
            resource::commands::LIST_CHANGED => self.resources_list_changed,
            resource::commands::UPDATED => {
                uri.is_some_and(|uri| self.resource_subscriptions.contains(uri))
            }
            _ => false,
        }
    }
}

/// `_meta` carried by every message on a subscription stream.
///
/// The value is the JSON-RPC id of the `subscriptions/listen` request that
/// opened the stream, so a client sharing one channel between several
/// subscriptions -- stdio always does -- can demultiplex them.
///
/// # Examples
/// ```
/// use neva::types::{RequestId, SubscriptionMeta};
///
/// let meta = SubscriptionMeta::new(RequestId::Number(1));
/// let json = serde_json::to_value(&meta).unwrap();
///
/// assert_eq!(json["io.modelcontextprotocol/subscriptionId"], 1);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionMeta {
    /// The id of the `subscriptions/listen` request this message belongs to.
    #[serde(rename = "io.modelcontextprotocol/subscriptionId")]
    pub subscription_id: RequestId,
}

impl SubscriptionMeta {
    /// Creates a new [`SubscriptionMeta`] for `subscription_id`.
    ///
    /// # Examples
    /// ```
    /// use neva::types::{RequestId, SubscriptionMeta};
    ///
    /// let meta = SubscriptionMeta::new(RequestId::Number(7));
    /// assert_eq!(meta.subscription_id, RequestId::Number(7));
    /// ```
    #[inline]
    pub fn new(subscription_id: RequestId) -> Self {
        Self { subscription_id }
    }
}

impl From<RequestId> for SubscriptionMeta {
    #[inline]
    fn from(subscription_id: RequestId) -> Self {
        Self::new(subscription_id)
    }
}

/// Parameters of a `subscriptions/listen` request.
///
/// # Examples
/// ```
/// use neva::types::{SubscriptionFilter, SubscriptionsListenRequestParams};
///
/// let params = SubscriptionsListenRequestParams::new(
///     SubscriptionFilter::new().with_tools_changed(),
/// );
///
/// assert!(params.notifications.tools_list_changed);
/// ```
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SubscriptionsListenRequestParams {
    /// The notification categories the client opts in to.
    pub notifications: SubscriptionFilter,

    /// Metadata reserved by MCP for protocol-level metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<RequestParamsMeta>,
}

impl SubscriptionsListenRequestParams {
    /// Creates new params opting in to `notifications`.
    ///
    /// # Examples
    /// ```
    /// use neva::types::{SubscriptionFilter, SubscriptionsListenRequestParams};
    ///
    /// let params = SubscriptionsListenRequestParams::new(SubscriptionFilter::new());
    /// assert!(params.notifications.is_empty());
    /// ```
    #[inline]
    pub fn new(notifications: SubscriptionFilter) -> Self {
        Self {
            notifications,
            meta: None,
        }
    }
}

impl From<SubscriptionFilter> for SubscriptionsListenRequestParams {
    #[inline]
    fn from(notifications: SubscriptionFilter) -> Self {
        Self::new(notifications)
    }
}

/// Parameters of a `notifications/subscriptions/acknowledged` notification.
///
/// Sent as the first message on a subscription, reporting the subset of the
/// requested filter the server agreed to honor.
///
/// # Examples
/// ```
/// use neva::types::{
///     RequestId, SubscriptionFilter, SubscriptionsAcknowledgedNotificationParams,
/// };
///
/// let params = SubscriptionsAcknowledgedNotificationParams::new(
///     RequestId::Number(1),
///     SubscriptionFilter::new().with_tools_changed(),
/// );
///
/// assert!(params.notifications.tools_list_changed);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionsAcknowledgedNotificationParams {
    /// The subset of the requested filter the server honors.
    pub notifications: SubscriptionFilter,

    /// Carries the subscription id (see [`SubscriptionMeta`]).
    #[serde(rename = "_meta")]
    pub meta: SubscriptionMeta,
}

impl SubscriptionsAcknowledgedNotificationParams {
    /// Creates acknowledgment params for `subscription_id`.
    ///
    /// # Examples
    /// ```
    /// use neva::types::{
    ///     RequestId, SubscriptionFilter, SubscriptionsAcknowledgedNotificationParams,
    /// };
    ///
    /// let params = SubscriptionsAcknowledgedNotificationParams::new(
    ///     RequestId::Number(1),
    ///     SubscriptionFilter::new(),
    /// );
    ///
    /// assert_eq!(params.meta.subscription_id, RequestId::Number(1));
    /// ```
    #[inline]
    pub fn new(subscription_id: RequestId, notifications: SubscriptionFilter) -> Self {
        Self {
            notifications,
            meta: SubscriptionMeta::new(subscription_id),
        }
    }
}

/// Final result of a `subscriptions/listen` request: the subscription ended
/// gracefully.
///
/// A stream that closes without this result was dropped abruptly, which a
/// client **MAY** treat as a reason to reconnect and re-subscribe.
///
/// # Examples
/// ```
/// use neva::types::{RequestId, SubscriptionsListenResult};
///
/// let result = SubscriptionsListenResult::new(RequestId::Number(1));
/// assert_eq!(result.meta.subscription_id, RequestId::Number(1));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionsListenResult {
    /// Carries the subscription id (see [`SubscriptionMeta`]).
    #[serde(rename = "_meta")]
    pub meta: SubscriptionMeta,
}

impl SubscriptionsListenResult {
    /// Creates the graceful-close result for `subscription_id`.
    ///
    /// # Examples
    /// ```
    /// use neva::types::{RequestId, SubscriptionsListenResult};
    ///
    /// let result = SubscriptionsListenResult::new(RequestId::Number(1));
    /// assert_eq!(result.meta.subscription_id, RequestId::Number(1));
    /// ```
    #[inline]
    pub fn new(subscription_id: RequestId) -> Self {
        Self {
            meta: SubscriptionMeta::new(subscription_id),
        }
    }
}

#[cfg(feature = "server")]
impl FromHandlerParams for SubscriptionsListenRequestParams {
    #[inline]
    fn from_params(params: &HandlerParams) -> Result<Self, Error> {
        let req = Request::from_params(params)?;
        Self::from_request(req)
    }
}

#[cfg(feature = "server")]
impl IntoResponse for SubscriptionsListenResult {
    #[inline]
    fn into_response(self, req_id: RequestId) -> Response {
        match serde_json::to_value(self) {
            Ok(v) => Response::success(req_id, v),
            Err(err) => Response::error(req_id, err.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        PromptsCapability, ResourcesCapability, ServerCapabilities, ToolsCapability,
    };

    fn caps(
        tools: bool,
        prompts: bool,
        resources_list: bool,
        subscribe: bool,
    ) -> ServerCapabilities {
        ServerCapabilities {
            tools: Some(ToolsCapability {
                list_changed: tools,
            }),
            prompts: Some(PromptsCapability {
                list_changed: prompts,
            }),
            resources: Some(ResourcesCapability {
                list_changed: resources_list,
                subscribe,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn it_omits_unset_categories_on_the_wire() {
        // "Omitting a field is equivalent to not subscribing to that
        // notification type" -- an unset category must not travel as `false`,
        // so an acknowledgment can drop what the server does not honor.
        let filter = SubscriptionFilter::new().with_tools_changed();
        let json = serde_json::to_value(&filter).unwrap();

        assert_eq!(json["toolsListChanged"], serde_json::json!(true));
        assert!(json.get("promptsListChanged").is_none());
        assert!(json.get("resourcesListChanged").is_none());
        assert!(json.get("resourceSubscriptions").is_none());
    }

    #[test]
    fn it_roundtrips_filter() {
        let filter = SubscriptionFilter::new()
            .with_tools_changed()
            .with_prompts_changed()
            .with_resources_changed()
            .with_resources(["res://a", "res://b"]);

        let json = serde_json::to_string(&filter).unwrap();
        let back: SubscriptionFilter = serde_json::from_str(&json).unwrap();

        assert_eq!(filter, back);
    }

    #[test]
    fn it_deserializes_absent_fields_as_unsubscribed() {
        let filter: SubscriptionFilter = serde_json::from_str("{}").unwrap();
        assert!(filter.is_empty());
    }

    #[test]
    fn it_deduplicates_resource_uris() {
        let filter = SubscriptionFilter::new().with_resources(["res://a", "res://a"]);
        assert_eq!(filter.resource_subscriptions.len(), 1);
    }

    #[test]
    fn it_intersects_filters() {
        let requested = SubscriptionFilter::new()
            .with_tools_changed()
            .with_prompts_changed()
            .with_resources(["res://a", "res://b"]);
        let offered = SubscriptionFilter::new()
            .with_prompts_changed()
            .with_resources_changed()
            .with_resources(["res://b", "res://c"]);

        let accepted = requested.intersection(&offered);

        assert!(!accepted.tools_list_changed);
        assert!(accepted.prompts_list_changed);
        assert!(!accepted.resources_list_changed);
        assert_eq!(accepted.resource_subscriptions, [Uri::from("res://b")]);
    }

    #[test]
    fn it_checks_subset() {
        let requested = SubscriptionFilter::new()
            .with_tools_changed()
            .with_resource("res://a");

        assert!(requested.is_subset_of(&requested));
        assert!(
            SubscriptionFilter::new()
                .with_tools_changed()
                .is_subset_of(&requested)
        );
        assert!(SubscriptionFilter::new().is_subset_of(&requested));
        assert!(
            !SubscriptionFilter::new()
                .with_prompts_changed()
                .is_subset_of(&requested)
        );
        assert!(
            !SubscriptionFilter::new()
                .with_resource("res://b")
                .is_subset_of(&requested)
        );
    }

    #[test]
    fn it_narrows_to_advertised_capabilities() {
        let requested = SubscriptionFilter::new()
            .with_tools_changed()
            .with_prompts_changed()
            .with_resources_changed()
            .with_resource("res://a");

        let accepted = requested.supported_by(&caps(true, false, true, false));

        assert!(accepted.tools_list_changed);
        assert!(!accepted.prompts_list_changed);
        assert!(accepted.resources_list_changed);
        assert!(accepted.resource_subscriptions.is_empty());
    }

    #[test]
    fn it_narrows_to_nothing_without_capabilities() {
        let requested = SubscriptionFilter::new()
            .with_tools_changed()
            .with_resource("res://a");

        let accepted = requested.supported_by(&ServerCapabilities::default());

        assert!(accepted.is_empty());
    }

    #[test]
    fn it_keeps_resource_subscriptions_when_subscribe_is_advertised() {
        let requested = SubscriptionFilter::new().with_resource("res://a");
        let accepted = requested.supported_by(&caps(false, false, false, true));

        assert_eq!(accepted.resource_subscriptions, [Uri::from("res://a")]);
    }

    #[test]
    fn it_matches_only_subscribed_methods() {
        use crate::types::{prompt, resource, tool};

        let filter = SubscriptionFilter::new()
            .with_tools_changed()
            .with_resource("res://a");

        assert!(filter.matches(tool::commands::LIST_CHANGED, None));
        assert!(!filter.matches(prompt::commands::LIST_CHANGED, None));
        assert!(!filter.matches(resource::commands::LIST_CHANGED, None));
        assert!(filter.matches(resource::commands::UPDATED, Some(&Uri::from("res://a"))));
        assert!(!filter.matches(resource::commands::UPDATED, Some(&Uri::from("res://b"))));
        assert!(!filter.matches(resource::commands::UPDATED, None));
        assert!(!filter.matches("notifications/progress", None));
    }

    #[test]
    fn it_serializes_subscription_id_meta() {
        let params = SubscriptionsAcknowledgedNotificationParams::new(
            RequestId::Number(1),
            SubscriptionFilter::new().with_tools_changed(),
        );
        let json = serde_json::to_value(&params).unwrap();

        assert_eq!(json["_meta"][SUBSCRIPTION_ID_KEY], serde_json::json!(1));
        assert_eq!(json["notifications"]["toolsListChanged"], true);
    }

    #[test]
    fn it_serializes_graceful_close_result() {
        let result = SubscriptionsListenResult::new(RequestId::String("sub-1".into()));
        let json = serde_json::to_value(&result).unwrap();

        assert_eq!(json["_meta"][SUBSCRIPTION_ID_KEY], "sub-1");
    }

    #[test]
    fn it_parses_listen_params() {
        let json = r#"{"notifications":{"toolsListChanged":true,
            "resourceSubscriptions":["file:///project/config.json"]}}"#;
        let params: SubscriptionsListenRequestParams = serde_json::from_str(json).unwrap();

        assert!(params.notifications.tools_list_changed);
        assert_eq!(
            params.notifications.resource_subscriptions,
            [Uri::from("file:///project/config.json")]
        );
    }
}
