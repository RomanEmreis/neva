//! Macros for a client

use crate::Client;
use super::inventory;

/// Registrar unit for tools, resources, templates and prompts
#[derive(Debug)]
pub struct ItemRegistrar(pub fn(&mut Client));
inventory::collect!(ItemRegistrar);

impl ItemRegistrar {
    /// Registers a tool, prompt or resource template depending on what the [`crate::macros::ItemRegistrar`] holds
    fn register(&self, client: &mut Client) {
        self.0(client);
    }
}

impl Client {
    /// Registers all declared tools, prompts and resources
    pub(crate) fn register_methods(&mut self) {
        for registrar in inventory::iter::<ItemRegistrar> {
            registrar.register(self);
        }
    }
}