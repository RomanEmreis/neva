//! Macros for server

use crate::App;
use super::inventory;

/// Registrar unit for tools, resources, templates and prompts
#[derive(Debug)]
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
    pub(crate) fn register_methods(&mut self) {
        for registrar in inventory::iter::<ItemRegistrar> {
            registrar.register(self);
        }
    }
}