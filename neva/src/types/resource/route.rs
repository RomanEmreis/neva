//! A set of route handling tools

use super::ReadResourceResult;
use crate::app::handler::RequestHandler;
use crate::types::Uri;
use std::ops::Deref;

const OPEN_BRACKET: char = '{';
const CLOSE_BRACKET: char = '}';

/// Represents route path node
pub(super) struct RouteNode {
    path: Box<str>,
    node: Box<Route>
}

/// A data structure for easy insert and search handler by route template
pub(crate) struct Route {
    static_routes: Vec<RouteNode>,
    dynamic_route: Option<RouteNode>,
    handler: Option<ResourceHandler>
}

/// A handler function for a resource route
pub(crate) struct ResourceHandler {
    #[cfg(feature = "http-server")]
    pub(crate) template: String,
    handler: RequestHandler<ReadResourceResult>
}

impl RouteNode {
    /// Creates a new [`RouteNode`]
    #[inline]
    fn new(path: &str) -> Self {
        Self {
            node: Box::new(Route::new()),
            path: path.into()
        }
    }

    /// Compares two route entries
    #[inline(always)]
    fn cmp(&self, path: &str) -> std::cmp::Ordering {
        self.path
            .as_ref()
            .cmp(path)
    }
}

impl Deref for ResourceHandler {
    type Target = RequestHandler<ReadResourceResult>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.handler
    }
}

impl Default for Route {
    #[inline]
    fn default() -> Self {
        Route::new()
    }
}

impl Route {
    /// Create a new [`Route`]
    #[inline]
    pub(super) fn new() -> Self {
        Self {
            static_routes: Vec::new(),
            handler: None,
            dynamic_route: None,
        }
    }
    
    /// Inserts a route handler
    pub(crate) fn insert(
        &mut self,
        path: &Uri,
        _template: String,
        handler: RequestHandler<ReadResourceResult>
    ) {
        let mut current = self;
        let path_segments = path.parts()
            .expect("URI parts should be present");

        for segment in path_segments {
            if is_dynamic_segment(segment) {
                current = current.insert_dynamic_node(segment);
            } else {
                current = current.insert_static_node(segment);
            }
        }

        current.handler = Some(ResourceHandler {
            #[cfg(feature = "http-server")]
            template: _template.clone(),
            handler: handler.clone(),
        });
    }

    /// Searches for a route handler
    pub(crate) fn find(&self, path: &Uri) -> Option<(&ResourceHandler, Box<[String]>)> {
        let mut current = self;
        let mut params = Vec::new();
        let path_segments = path.parts()?;

        for segment in path_segments {
            if let Ok(i) = current.static_routes.binary_search_by(|r| r.cmp(segment)) {
                current = current.static_routes[i].node.as_ref();
                continue;
            }

            if let Some(next) = &current.dynamic_route {
                params.push(segment.into());
                current = next.node.as_ref();
                continue;
            }

            return None;
        }

        current.handler
            .as_ref()
            .map(|h| (h, params.into_boxed_slice()))
    }

    #[inline(always)]
    fn insert_static_node(&mut self, segment: &str) -> &mut Self {
        match self.static_routes.binary_search_by(|r| r.cmp(segment)) {
            Ok(i) => &mut self.static_routes[i].node,
            Err(i) => {
                self.static_routes.insert(i, RouteNode::new(segment));
                &mut self.static_routes[i].node
            }
        }
    }

    #[inline(always)]
    fn insert_dynamic_node(&mut self, segment: &str) -> &mut Self {
        self
            .dynamic_route
            .get_or_insert_with(|| RouteNode::new(segment))
            .node
            .as_mut()
    }
}

#[inline(always)]
fn is_dynamic_segment(segment: &str) -> bool {
    segment.starts_with(OPEN_BRACKET) &&
        segment.ends_with(CLOSE_BRACKET)
}

#[cfg(test)]
mod tests {
    use crate::types::resource::template::ResourceFunc;
    use crate::types::{ResourceContents, Uri};
    use super::*;
    
    #[test]
    fn it_inserts_and_finds() {
        let uri1: Uri = "res://path/to/{resource}".into();
        let handler1 = ResourceFunc::new(|uri: Uri| async move {
            ResourceContents::new(uri)
                .with_mime("text/plain")
                .with_text("some text 1")
        });

        let uri2: Uri = "res://another/path/to/{resource}".into();
        let handler2 = ResourceFunc::new(|uri: Uri| async move {
            ResourceContents::new(uri)
                .with_mime("text/plain")
                .with_text("some text 2")
        });
        
        let mut route = Route::default();
        route.insert(&uri1, "templ_1".into(), handler1);
        route.insert(&uri2, "templ_2".into(), handler2);
        
        assert!(route.find(&uri1).is_some());
        assert!(route.find(&uri2).is_some());
    }
}