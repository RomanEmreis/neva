//! What the connected server said it can do.
//!
//! Every probe reads the capabilities captured during the handshake, so they
//! answer `false` on a client that has not connected yet rather than failing.
//! They exist so a caller can ask before it calls, and so the client itself can
//! refuse a request the server never advertised.

use super::*;

impl Client {
    /// Returns whether the server is configured to send the "notifications/resources/updated"
    #[inline]
    pub(super) fn is_resource_subscription_supported(&self) -> bool {
        self.server_capabilities
            .as_ref()
            .and_then(|cap| cap.resources.as_ref())
            .is_some_and(|res| res.subscribe)
    }

    /// Returns whether the server is configured to send the "notifications/resources/list_changed"
    #[inline]
    pub(super) fn is_resource_list_changed_supported(&self) -> bool {
        self.server_capabilities
            .as_ref()
            .and_then(|cap| cap.resources.as_ref())
            .is_some_and(|res| res.list_changed)
    }

    /// Returns whether the server is configured to send the "notifications/tools/list_changed"
    #[inline]
    pub(super) fn is_tools_list_changed_supported(&self) -> bool {
        self.server_capabilities
            .as_ref()
            .and_then(|cap| cap.tools.as_ref())
            .is_some_and(|tool| tool.list_changed)
    }

    /// Returns whether the server is configured to send the "notifications/prompts/list_changed"
    #[inline]
    pub(super) fn is_prompts_list_changed_supported(&self) -> bool {
        self.server_capabilities
            .as_ref()
            .and_then(|cap| cap.prompts.as_ref())
            .is_some_and(|prompt| prompt.list_changed)
    }

    /// Returns whether the client has elicitation capabilities
    #[inline]
    #[cfg(feature = "legacy-spec")]
    pub(super) fn is_elicitation_supported(&self) -> bool {
        self.options.elicitation_capability.as_ref().is_some()
    }

    /// Returns whether the client has task augmentation capabilities
    #[inline]
    #[cfg(feature = "tasks")]
    pub(super) fn is_client_supports_tasks(&self) -> bool {
        self.options.tasks_capability.as_ref().is_some()
    }

    /// Resolves the server's tasks capability from the negotiated server
    /// capabilities. Pre-2026-07-28 it is the top-level `tasks` field; under
    /// MCP 2026-07-28 tasks are an extension, so it is read from
    /// `capabilities.extensions["io.modelcontextprotocol/tasks"]`.
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(super) fn server_tasks_capability(&self) -> Option<crate::types::ServerTasksCapability> {
        self.server_capabilities
            .as_ref()
            .and_then(|c| c.tasks.clone())
    }

    /// Resolves the server's tasks capability from the negotiated server
    /// capabilities (MCP 2026-07-28 build).
    ///
    /// Tasks are an extension here, so the one place they can be advertised is
    /// `capabilities.extensions["io.modelcontextprotocol/tasks"]`. The
    /// pre-2026-07-28 top-level `tasks` field is deliberately not read: it can
    /// only come from a peer reached through the dual-mode fallback, whose task
    /// protocol this build does not speak (see
    /// [`Self::is_server_supports_tasks`]), so resolving it would only ever
    /// promise support that cannot be delivered.
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    pub(super) fn server_tasks_capability(&self) -> Option<crate::types::ServerTasksCapability> {
        self.server_capabilities
            .as_ref()?
            .extensions
            .as_ref()
            .and_then(|ext| ext.get(crate::types::task::TASKS_EXTENSION_ID))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Returns whether the server has task augmentation capabilities
    #[inline]
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(super) fn is_server_supports_tasks(&self) -> bool {
        self.server_tasks_capability().is_some()
    }

    /// Returns whether the server has task augmentation capabilities
    ///
    /// A peer reached through the dual-mode fallback speaks the *legacy* task
    /// protocol -- different method set (`tasks/result`, `tasks/list`), a
    /// nested `CreateTaskResult`, a differently named status notification --
    /// and none of that wire surface is compiled into this build. It is
    /// reported as unsupported rather than answered with 2026-07-28 messages
    /// it cannot read; talking tasks to a legacy server needs a `legacy-spec`
    /// build, or the peers must simply agree on a generation. The peer check
    /// is belt-and-braces on top of
    /// [`Self::server_tasks_capability`](Self::server_tasks_capability) only
    /// reading the 2026-07-28 form: it also covers a peer that advertises the
    /// extension and then falls back.
    #[inline]
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    pub(super) fn is_server_supports_tasks(&self) -> bool {
        !self.is_legacy_peer() && self.server_tasks_capability().is_some()
    }

    /// Returns whether the client supports cancelling tasks
    #[inline]
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(super) fn is_client_support_cancelling_tasks(&self) -> bool {
        self.options
            .tasks_capability
            .as_ref()
            .is_some_and(|c| c.cancel.is_some())
    }

    /// Returns whether the server supports cancelling tasks
    #[inline]
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(super) fn is_server_support_cancelling_tasks(&self) -> bool {
        self.server_tasks_capability()
            .is_some_and(|c| c.cancel.is_some())
    }

    /// Returns whether the server supports retrieving a task list
    #[inline]
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(super) fn is_server_support_task_list(&self) -> bool {
        self.server_tasks_capability()
            .is_some_and(|c| c.list.is_some())
    }

    /// Returns whether the client supports retrieving a task list
    #[inline]
    #[cfg(all(feature = "tasks", feature = "legacy-spec"))]
    pub(super) fn is_client_support_task_list(&self) -> bool {
        self.options
            .tasks_capability
            .as_ref()
            .is_some_and(|c| c.list.is_some())
    }

    /// Returns whether the server supports task-augmented tools
    ///
    /// Under MCP 2026-07-28 the Tasks extension capability carries no
    /// per-request settings: a peer that advertises the extension at all
    /// accepts task-augmented requests, and the server decides per request
    /// whether to defer. A peer that fell back to the legacy protocol is
    /// excluded -- see [`Self::is_server_supports_tasks`].
    #[inline]
    #[cfg(all(feature = "tasks", not(feature = "legacy-spec")))]
    pub(super) fn is_server_support_call_tool_with_tasks(&self) -> bool {
        self.is_server_supports_tasks()
    }
}

/// Task support in a 2026-07-28 build means one thing: the peer advertised the
/// extension *and* stayed on the 2026-07-28 protocol. Generations do not mix
/// for tasks -- run a `legacy-spec` build against a legacy server.
#[cfg(all(test, feature = "tasks", not(feature = "legacy-spec")))]
mod fallback_tasks_capability_tests {
    use super::*;
    use crate::types::ServerCapabilities;
    use serde_json::json;

    fn client_with_capabilities(caps: serde_json::Value) -> Client {
        let mut client = Client::new();
        client.server_capabilities =
            Some(serde_json::from_value::<ServerCapabilities>(caps).expect("valid capabilities"));
        client
    }

    /// The legacy top-level `tasks` field can only reach this build from a
    /// fallback peer, whose task protocol it does not speak. Reading it would
    /// promise support that ends in 2026-07-28 messages the peer cannot read.
    #[test]
    fn a_legacy_top_level_tasks_capability_is_not_support() {
        let client = client_with_capabilities(json!({
            "tools": {},
            "tasks": { "requests": { "tools": { "call": {} } } }
        }));
        assert!(!client.is_server_supports_tasks());
        assert!(!client.is_server_support_call_tool_with_tasks());
    }

    /// And a peer that advertised the 2026-07-28 extension but then negotiated
    /// the legacy protocol is no different.
    #[test]
    fn a_fallback_peer_reports_no_task_support() {
        let client = client_with_capabilities(json!({
            "tools": {},
            "extensions": {
                "io.modelcontextprotocol/tasks": { "requests": { "tools": { "call": {} } } }
            }
        }));
        assert!(client.is_server_supports_tasks());

        client.options.peer_mode.set_legacy();
        assert!(!client.is_server_supports_tasks());
        assert!(!client.is_server_support_call_tool_with_tasks());
    }

    #[test]
    fn the_extension_tasks_capability_resolves() {
        let client = client_with_capabilities(json!({
            "tools": {},
            "extensions": {
                "io.modelcontextprotocol/tasks": { "requests": { "tools": { "call": {} } } }
            }
        }));
        assert!(client.is_server_supports_tasks());
    }

    #[test]
    fn no_tasks_capability_resolves_to_none() {
        let client = client_with_capabilities(json!({ "tools": {} }));
        assert!(!client.is_server_supports_tasks());
    }
}
