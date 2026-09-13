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
    node: Box<Route>,
}

/// A data structure for easy insert and search handler by route template
pub(crate) struct Route {
    static_routes: Vec<RouteNode>,
    dynamic_route: Option<RouteNode>,
    handler: Option<ResourceHandler>,
}

/// A handler function for a resource route
pub(crate) struct ResourceHandler {
    /// The resource template this route was registered from, by name -- or,
    /// for a `ui://` resource, which no template backs, its URI.
    ///
    /// `http-server` copies the template's requirement onto
    /// [`Self::required`] when the server starts; `apps` reads the `_meta.ui`
    /// block a `#[resource(ui_meta = ..)]` put there.
    #[cfg(any(feature = "http-server", feature = "apps"))]
    pub(crate) template: String,

    /// What a caller must hold to read this route.
    ///
    /// The one place `resources/read` looks, whatever registered the route: a
    /// template's `with_roles` / `with_permissions` land here when the server
    /// starts, a UI resource's when it is materialized.
    #[cfg(feature = "http-server")]
    pub(crate) required: crate::transport::http::core::auth::RequiredClaims,

    handler: RequestHandler<ReadResourceResult>,
}

impl RouteNode {
    /// Creates a new [`RouteNode`]
    #[inline]
    fn new(path: &str) -> Self {
        Self {
            node: Box::new(Route::new()),
            path: path.into(),
        }
    }

    /// Compares two route entries
    #[inline(always)]
    fn cmp(&self, path: &str) -> std::cmp::Ordering {
        self.path.as_ref().cmp(path)
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

    /// Inserts a route handler, returning it so a caller can state the route's
    /// own requirements.
    pub(crate) fn insert(
        &mut self,
        path: &Uri,
        _template: String,
        handler: RequestHandler<ReadResourceResult>,
    ) -> &mut ResourceHandler {
        let mut current = self;
        let path_segments = path.parts().expect("URI parts should be present");

        for segment in path_segments {
            if is_dynamic_segment(segment) {
                current = current.insert_dynamic_node(segment);
            } else {
                current = current.insert_static_node(segment);
            }
        }

        current.handler.insert(ResourceHandler {
            #[cfg(any(feature = "http-server", feature = "apps"))]
            template: _template,
            #[cfg(feature = "http-server")]
            required: Default::default(),
            handler,
        })
    }

    /// Visits every route handler, for a pass over the whole table.
    #[cfg(feature = "http-server")]
    pub(crate) fn for_each_handler_mut(&mut self, f: &mut impl FnMut(&mut ResourceHandler)) {
        if let Some(handler) = self.handler.as_mut() {
            f(handler);
        }
        self.static_routes
            .iter_mut()
            .chain(self.dynamic_route.as_mut())
            .for_each(|next| next.node.for_each_handler_mut(f));
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

        current
            .handler
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
        self.dynamic_route
            .get_or_insert_with(|| RouteNode::new(segment))
            .node
            .as_mut()
    }
}

#[inline(always)]
fn is_dynamic_segment(segment: &str) -> bool {
    segment.starts_with(OPEN_BRACKET) && segment.ends_with(CLOSE_BRACKET)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resource::template::ResourceFunc;
    use crate::types::{ResourceContents, Uri};

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

    #[cfg(feature = "http-server")]
    #[test]
    fn a_pass_over_the_table_visits_every_handler() {
        let handler = || ResourceFunc::new(|uri: Uri| async move { ResourceContents::new(uri) });

        let mut route = Route::default();
        for (uri, template) in [
            ("res://a", "a"),
            ("res://a/b", "ab"),
            ("res://a/{id}", "a_id"),
            ("res://a/{id}/c", "a_id_c"),
            ("ui://z/app.html", "ui"),
        ] {
            route.insert(&uri.into(), template.into(), handler());
        }

        let mut seen = Vec::new();
        route.for_each_handler_mut(&mut |h| seen.push(h.template.clone()));
        seen.sort();

        assert_eq!(seen, ["a", "a_id", "a_id_c", "ab", "ui"]);
    }
}
