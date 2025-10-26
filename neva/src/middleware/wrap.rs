//! MCP middleware wrappers

use std::future::Future;
use crate::{
    middleware::{make_fn::{make_mw, make_on, make_on_command}, MwContext, Next},
    types::{Message, Response},
    App
};

impl App {
    /// Registers a global middleware
    pub fn wrap<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_mw(middleware));
        self
    }

    /// Registers a global middleware that runs only 
    /// if the MCP server received a notification message
    pub fn wrap_notification<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on(
            middleware,
            |msg| msg.is_notification()));
        self
    }

    /// Registers a global middleware that runs only 
    /// if the MCP server received a request message
    pub fn wrap_request<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on(
            middleware,
            |msg| msg.is_request()));
        self
    }

    /// Registers a global middleware that runs only 
    /// if the MCP server received a response message
    pub fn wrap_response<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on(
            middleware,
            |msg| msg.is_response()));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `tools/call` request
    pub fn wrap_tools<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::tool::commands::CALL));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `prompts/get` request
    pub fn wrap_prompts<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::prompt::commands::GET));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `resources/read` request
    pub fn wrap_resources<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::resource::commands::READ));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `resources/list` request
    pub fn wrap_list_resources<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::resource::commands::LIST));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `resources/templates/list` request
    pub fn wrap_list_resource_templates<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::resource::commands::TEMPLATES_LIST));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `tools/list` request
    pub fn wrap_list_tools<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::tool::commands::LIST));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `prompts/list` request
    pub fn wrap_list_prompts<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::types::prompt::commands::LIST));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received an `initialize` request
    pub fn wrap_init<F, R>(mut self, middleware: F) -> Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            crate::commands::INIT));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received the command with the `name` request
    pub fn wrap_command<F, R>(&mut self, name: &'static str, middleware: F) -> &mut Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on_command(
            middleware,
            name));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `tools/call` request
    pub fn wrap_tool<F, R>(&mut self, name: &'static str, middleware: F) -> &mut Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on(
            middleware,
            move |msg| {
                if let Message::Request(req) = msg {
                    req.method == crate::types::tool::commands::CALL &&
                        req.params
                            .as_ref()
                            .is_some_and(|p| p.get("name")
                                .is_some_and(|n| n == name))
                } else {
                    false
                }
            }));
        self
    }

    /// Registers a middleware that runs only 
    /// if the MCP server received a `prompt/get` request
    pub fn wrap_prompt<F, R>(&mut self, name: &'static str, middleware: F) -> &mut Self
    where
        F: Fn(MwContext, Next) -> R + Clone + Send + Sync + 'static,
        R: Future<Output = Response> + Send + 'static,
    {
        self.options.add_middleware(make_on(
            middleware,
            move |msg| {
                if let Message::Request(req) = msg {
                    req.method == crate::types::prompt::commands::GET &&
                        req.params
                            .as_ref()
                            .is_some_and(|p| p.get("name")
                                .is_some_and(|n| n == name))
                } else {
                    false
                }
            }));
        self
    }
}