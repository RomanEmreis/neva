//! What a toolbox offers a model: the filters and renames its caller set, and
//! the snapshot they made of the last listing.

use super::convert;
use crate::error::{Error, ErrorCode};
use crate::types::Tool;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::{PoisonError, RwLock};

/// Keeps a tool in the offer, seeing it under the server's name.
pub(super) type Filter = Box<dyn Fn(&Tool) -> bool + Send + Sync>;

/// Makes the name a tool is offered under from the one before it.
pub(super) type Rename = Box<dyn Fn(&str) -> String + Send + Sync>;

/// The tools offered to a model, and how they were chosen.
#[derive(Default)]
pub(super) struct Offer {
    filters: Vec<Filter>,
    renames: Vec<Rename>,
    snapshot: RwLock<Snapshot>,
}

/// What one listing offered.
#[derive(Default)]
struct Snapshot {
    /// The descriptors, as the model is offered them.
    tools: Vec<::svir::Tool>,
    /// The name a model calls -> the name the server knows.
    names: HashMap<String, String>,
}

impl Debug for Offer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let snapshot = self.snapshot.read().unwrap_or_else(PoisonError::into_inner);
        f.debug_struct("Offer")
            .field("filters", &self.filters.len())
            .field("renames", &self.renames.len())
            .field("tools", &snapshot.names.keys())
            .finish()
    }
}

impl Offer {
    /// Adds a filter: a tool is offered only if every filter keeps it.
    pub(super) fn filter(&mut self, filter: Filter) {
        self.filters.push(filter);
    }

    /// Adds a rename, given the name the renames before it made.
    pub(super) fn rename(&mut self, rename: Rename) {
        self.renames.push(rename);
    }

    /// Replaces the snapshot with what `listed` offers.
    ///
    /// Leaves out what the model must not see or could never call, then what
    /// the filters leave out. Fails, keeping the previous snapshot, if a tool
    /// that would be offered has a name a model API cannot carry or shares
    /// its name with another: it is refused rather than renamed behind the
    /// caller's back.
    pub(super) fn replace(&self, listed: impl IntoIterator<Item = Tool>) -> Result<(), Error> {
        let mut snapshot = Snapshot::default();
        let offered = listed
            .into_iter()
            .filter(|tool| offerable(tool) && self.filters.iter().all(|keep| keep(tool)));

        for mut tool in offered {
            let name = self
                .renames
                .iter()
                .fold(Cow::Borrowed(tool.name.as_str()), |name, rename| {
                    Cow::Owned(rename(&name))
                })
                .into_owned();

            if snapshot.names.contains_key(&name) {
                return Err(Error::new(
                    ErrorCode::InvalidParams,
                    format!(
                        "Tool `{}` is offered as `{name}`, a name already taken",
                        tool.name
                    ),
                ));
            }

            // The descriptor carries the offered name; the server's moves
            // into the map that finds it again.
            let server = std::mem::take(&mut tool.name);
            snapshot.names.insert(name.clone(), server);
            snapshot.tools.push(convert::descriptor(tool, name)?);
        }

        *self
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner) = snapshot;

        Ok(())
    }

    /// The descriptors, as the model is offered them.
    pub(super) fn tools(&self) -> Vec<::svir::Tool> {
        self.snapshot
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .tools
            .clone()
    }

    /// The server's name of the tool a model calls `name`, if it is offered.
    ///
    /// Owned: the lock is not held across the call that follows, and the
    /// request owns its tool name anyway.
    pub(super) fn server_name(&self, name: &str) -> Option<String> {
        self.snapshot
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .names
            .get(name)
            .cloned()
    }
}

/// Whether a tool can be offered at all: the model may see it, and a call it
/// makes could be answered.
fn offerable(tool: &Tool) -> bool {
    visible(tool) && !needs_task(tool)
}

/// Whether the model may see `tool`: MCP Apps can hide a tool from it.
#[cfg(feature = "apps")]
#[inline]
fn visible(tool: &Tool) -> bool {
    tool.is_model_visible()
}

/// Without the `apps` feature nothing is read of MCP Apps, as in the rest of
/// neva: every tool the server lists is an ordinary tool.
#[cfg(not(feature = "apps"))]
#[inline]
fn visible(_: &Tool) -> bool {
    true
}

/// Whether `tool` can only be called as a task. The bridge calls tools
/// plainly, so such a tool would refuse every call the model made.
#[cfg(feature = "tasks")]
#[inline]
fn needs_task(tool: &Tool) -> bool {
    tool.task_support() == Some(crate::types::tool::TaskSupport::Required)
}

/// Without the `tasks` feature nothing is read of task support.
#[cfg(not(feature = "tasks"))]
#[inline]
fn needs_task(_: &Tool) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str) -> Tool {
        serde_json::from_value(json!({
            "name": name,
            "inputSchema": { "type": "object", "properties": {} }
        }))
        .expect("a tool")
    }

    fn names(offer: &Offer) -> Vec<String> {
        offer.tools().into_iter().map(|tool| tool.name).collect()
    }

    /// Filters add up, as `Iterator::filter` does, and see the server's names
    /// whatever renames come before them.
    #[test]
    fn filters_and_renames_add_up() {
        let mut offer = Offer::default();
        offer.rename(Box::new(|name| name.replace('.', "_")));
        offer.filter(Box::new(|tool| tool.name != "files.delete"));
        offer.rename(Box::new(|name| format!("gh_{name}")));
        offer.filter(Box::new(|tool| tool.name != "issues"));

        let listed = [tool("files.read"), tool("files.delete"), tool("issues")];
        offer.replace(listed).expect("a snapshot");

        assert_eq!(names(&offer), ["gh_files_read"]);
        assert_eq!(
            offer.server_name("gh_files_read").as_deref(),
            Some("files.read")
        );
        assert_eq!(offer.server_name("files.read"), None);
    }

    /// A failed snapshot leaves the previous one in place.
    #[test]
    fn a_name_taken_twice_keeps_the_previous_snapshot() {
        let mut offer = Offer::default();
        offer.replace([tool("one")]).expect("a snapshot");

        offer.rename(Box::new(|_| "same".into()));
        let err = offer
            .replace([tool("one"), tool("two")])
            .expect_err("two tools under one name");
        assert!(err.to_string().contains("already taken"), "{err}");
        assert_eq!(names(&offer), ["one"]);
    }

    /// The bridge calls tools plainly; one that can only run as a task would
    /// refuse every call.
    #[cfg(feature = "tasks")]
    #[test]
    fn a_tool_that_needs_a_task_is_not_offered() {
        let task_only: Tool = serde_json::from_value(json!({
            "name": "long",
            "inputSchema": { "type": "object", "properties": {} },
            "execution": { "taskSupport": "required" }
        }))
        .expect("a tool");
        let optional: Tool = serde_json::from_value(json!({
            "name": "either",
            "inputSchema": { "type": "object", "properties": {} },
            "execution": { "taskSupport": "optional" }
        }))
        .expect("a tool");

        let offer = Offer::default();
        offer.replace([task_only, optional]).expect("a snapshot");
        assert_eq!(names(&offer), ["either"]);
    }
}
