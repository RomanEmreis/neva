//! Represents a generic-collection implementation that can be mutated during runtime

use std::collections::HashMap;
use tokio::sync::RwLock;
use crate::error::{Error, ErrorCode};

/// Generic collection with 2 states:
/// - [`Collection::Init`] - initialization state can be mutated without blocking
/// - [`Collection::Runtime`] - runtime state, the collection can be read by multiple readers 
/// and will blocked by only one writer
pub(crate) enum Collection<T: Clone> {
    Init(HashMap<String, T>),
    Runtime(RwLock<HashMap<String, T>>)
}

impl<T: Clone> Collection<T> {
    /// Creates a new [`Collection`] in [`Collection::Init`] state
    pub(crate) fn new() -> Self {
        Self::Init(HashMap::new())
    }

    /// Turns the [`Collection`] into [`Collection::Runtime`] state
    #[inline]
    pub(crate) fn into_runtime(self) -> Self {
        if let Self::Init(map) = self  {
            Self::Runtime(RwLock::new(map))
        } else {
            self
        }
    }

    /// Returns a copy of a `value` from the collection by its `key`
    #[inline]
    pub(crate) async fn get(&self, key: &str) -> Option<T> {
        match self {
            Self::Init(map) => map.get(key).cloned(),
            Self::Runtime(lock) => {
                lock.read()
                    .await
                    .get(key)
                    .cloned()
            }
        }
    }

    /// Inserts a key-value pair into this [`Collection`] when it in [`Collection::Runtime`] state.
    /// 
    /// For the [`Collection::Init`] state use the `as_mut().insert()` method.
    #[inline]
    pub(crate) async fn insert(&self, key: String, value: T) -> Result<(), Error> {
        match self {
            Self::Init(_) => return Err(Error::new(
                ErrorCode::InternalError, 
                "Attempt to insert a value during runtime when collection is in the init state")),
            Self::Runtime(lock) => {
                lock.write()
                    .await
                    .insert(key, value)
            }
        };
        Ok(())
    }

    /// Return a list of values
    #[inline]
    pub(crate) async fn values(&self) -> Vec<T> {
        match self {
            Self::Init(map) => map
                .values()
                .cloned()
                .collect(),
            Self::Runtime(lock) => lock.read().await
                .values()
                .cloned()
                .collect()
        }
    }
}

impl<T: Clone> AsMut<HashMap<String, T>> for Collection<T> {
    #[inline]
    fn as_mut(&mut self) -> &mut HashMap<String, T> {
        if let Self::Init(map) = self {
            map
        } else {
            unreachable!()
        }
    }
}

impl<T: Clone> AsRef<HashMap<String, T>> for Collection<T> {
    #[inline]
    fn as_ref(&self) -> &HashMap<String, T> {
        if let Self::Init(map) = self {
            map
        } else {
            unreachable!()
        }
    }
}