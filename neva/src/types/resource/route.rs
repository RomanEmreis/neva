//! A set of route handling tools

use super::ReadResourceResult;
use crate::app::handler::RequestHandler;
use std::{borrow::Cow, collections::HashMap};
use std::ops::Deref;

const END_OF_ROUTE: &str = "";
const OPEN_BRACKET: char = '{';
const CLOSE_BRACKET: char = '}';

/// A data structure for easy insert and search handler by route template
pub(crate) enum Route {
    Static(HashMap<Cow<'static, str>, Route>),
    Dynamic(HashMap<Cow<'static, str>, Route>),
    Handler(ResourceHandler)
}

pub(crate) struct ResourceHandler {
    #[cfg(feature = "http-server")]
    pub(crate) template: String,
    handler: RequestHandler<ReadResourceResult>
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
        Route::Static(HashMap::new())
    }
}

impl Route {
    /// Inserts a route handler
    pub(crate) fn insert(
        &mut self,
        path_segments: &[Cow<'static, str>],
        _template: String,
        handler: RequestHandler<ReadResourceResult>
    ) {
        let mut current = self;
        for (index, segment) in path_segments.iter().enumerate() {
            let is_last = index == path_segments.len() - 1;
            let is_dynamic = Self::is_dynamic_segment(segment);

            current = match current {
                Route::Handler(_) => panic!("Attempt to insert a route under a handler"),
                Route::Static(map) | 
                Route::Dynamic(map) => {
                    let entry = map.entry(segment.clone()).or_insert_with(|| {
                        if is_dynamic {
                            Route::Dynamic(HashMap::new())
                        } else {
                            Route::Static(HashMap::new())
                        }
                    });

                    // Check if this segment is the last, and add the handler
                    if is_last {
                        // Assumes the inserted or existing route has HashMap as associated data
                        match entry {
                            Route::Dynamic(ref mut map) |
                            Route::Static(ref mut map) => {
                                map.insert(
                                    END_OF_ROUTE.into(),
                                    Route::Handler(ResourceHandler {
                                        #[cfg(feature = "http-server")]
                                        template: _template.clone(),
                                        handler: handler.clone(),
                                    })
                                );
                            },
                            _ => ()
                        }
                    }

                    entry // Continue traversing or inserting into this entry
                },
            };
        }
    }

    /// Searches for a route handler
    pub(crate) fn find(&self, path_segments: &[Cow<'static, str>]) -> Option<(&Route, Box<[Cow<'static, str>]>)> {
        let mut current = Some(self);
        let mut params = Vec::with_capacity(4);
        for (index, segment) in path_segments.iter().enumerate() {
            let is_last = index == path_segments.len() - 1;

            current = match current {
                Some(Route::Static(map)) | 
                Some(Route::Dynamic(map)) => {
                    // Trying direct match first
                    let direct_match = map.get(segment);

                    // If no direct match, try dynamic route resolution
                    let resolved_route = direct_match.or_else(|| {
                        map.iter()
                            .filter(|(key, _)| Self::is_dynamic_segment(key))
                            .map(|(key, route)| {
                                if key
                                    .strip_prefix(OPEN_BRACKET)
                                    .and_then(|k| k.strip_suffix(CLOSE_BRACKET))
                                    .is_some() {
                                    params.push(segment.clone());
                                }
                                route
                            })
                            .next()
                    });

                    // Retrieve handler or further route if this is the last segment
                    if let Some(route) = resolved_route {
                        if is_last {
                            match route {
                                Route::Dynamic(inner_map) | Route::Static(inner_map) => {
                                    // Attempt to get handler directly if no further routing is possible
                                    inner_map.get(END_OF_ROUTE).or(Some(route))
                                },
                                handler @ Route::Handler(_) => Some(handler), // Direct handler return
                            }
                        } else {
                            Some(route) // Continue on non-terminal routes
                        }
                    } else {
                        None // No route resolved
                    }
                },
                _ => None,
            };
        }
        current.map(|route| (route, params.into_boxed_slice()))
    }

    #[inline]
    fn is_dynamic_segment(segment: &str) -> bool {
        segment.starts_with(OPEN_BRACKET) && 
        segment.ends_with(CLOSE_BRACKET)
    }
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
        route.insert(uri1.as_vec().as_slice(), "templ_1".into(), handler1);
        route.insert(uri2.as_vec().as_slice(), "templ_2".into(), handler2);
        
        let (h1, _) = route.find(uri1.as_vec().as_slice()).unwrap();
        let (h2, _) = route.find(uri2.as_vec().as_slice()).unwrap();
        
        matches!(h1, Route::Handler(_));
        matches!(h2, Route::Handler(_));
    }
}