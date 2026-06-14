//! The built-in Tasks extension (`io.modelcontextprotocol/tasks`).

use super::Extension;
use crate::app::App;

/// The built-in Tasks extension (`io.modelcontextprotocol/tasks`).
///
/// First consumer of [`Extension`]. It is normally configured through the
/// `with_tasks` thin wrapper
/// ([`McpOptions::with_tasks`](crate::app::options::McpOptions::with_tasks)),
/// which keeps the existing ergonomics, but it can also be registered directly
/// via [`App::with_extension`].
///
/// # Example
///
/// ```rust,ignore
/// use neva::App;
/// use neva::app::extension::TasksExtension;
/// use neva::types::ServerTasksCapability;
///
/// let app = App::new()
///     .with_extension(TasksExtension::new(ServerTasksCapability::default()));
/// ```
#[derive(Debug, Clone)]
pub struct TasksExtension {
    capability: crate::types::ServerTasksCapability,
}

impl TasksExtension {
    /// The reverse-DNS id of the Tasks extension.
    pub const ID: &'static str = crate::types::task::TASKS_EXTENSION_ID;

    /// Creates a new Tasks extension advertising `capability`.
    pub fn new(capability: crate::types::ServerTasksCapability) -> Self {
        Self { capability }
    }
}

impl Extension for TasksExtension {
    #[inline]
    fn id(&self) -> &'static str {
        Self::ID
    }

    #[inline]
    fn capability(&self) -> serde_json::Value {
        serde_json::to_value(&self.capability).unwrap_or_default()
    }

    fn register(self, app: &mut App) {
        use crate::types::task::commands;
        app.options.set_tasks_capability(self.capability);
        app.map_handler(commands::LIST, App::tasks);
        app.map_handler(commands::GET, App::task);
        app.map_handler(commands::CANCEL, App::cancel_task);
        app.map_handler(commands::RESULT, App::task_result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ServerTasksCapability;

    #[test]
    fn tasks_extension_id_is_reverse_dns() {
        let ext = TasksExtension::new(ServerTasksCapability::default());
        assert_eq!(ext.id(), "io.modelcontextprotocol/tasks");
        assert_eq!(TasksExtension::ID, "io.modelcontextprotocol/tasks");
    }

    #[test]
    fn tasks_extension_capability_serializes_to_object() {
        let ext = TasksExtension::new(ServerTasksCapability::default());
        assert!(ext.capability().is_object());
    }

    #[test]
    fn with_extension_records_capability_in_registry() {
        let app = App::new().with_extension(TasksExtension::new(ServerTasksCapability::default()));
        let exts = app
            .options
            .extensions()
            .expect("extension capability should be registered");
        assert!(exts.contains_key(TasksExtension::ID));
    }

    #[test]
    fn with_tasks_thin_wrapper_registers_extension() {
        let app = App::new().with_options(|opt| opt.with_tasks(|t| t.with_all()));
        let exts = app
            .options
            .extensions()
            .expect("with_tasks should register the tasks extension");
        assert!(exts.contains_key(TasksExtension::ID));
    }
}
