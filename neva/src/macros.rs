//! Utilities for macros

pub use inventory;
use crate::App;

/// Registrar unit for tools, resources, templates and prompts
pub struct ItemRegistrar(pub fn(&mut App));
inventory::collect!(ItemRegistrar);

impl ItemRegistrar {
    /// Registers a tool, prompt or resource template depending on what the [`ItemRegistrar`] holds
    fn register(&self, app: &mut App) {
        self.0(app);
    }
}

impl App {
    /// Registers all declared tools, prompts and resources
    pub(super) fn register_methods(&mut self) {
        for registrar in inventory::iter::<ItemRegistrar> {
            registrar.register(self);
        }
    }
}