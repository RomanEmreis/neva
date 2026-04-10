//! Represents a generic-collection implementation that can be mutated during runtime

use crate::error::{Error, ErrorCode};
use crate::types::Cursor;
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Generic collection with 2 states:
/// - [`Collection::Init`] - initialization state can be mutated without blocking
/// - [`Collection::Runtime`] - runtime state, the collection can be read by multiple readers and will blocked by only one writer
pub(crate) enum Collection<T: Clone> {
    Init(HashMap<String, T>),
    Runtime(RwLock<HashMap<String, T>>),
}

impl<T: Clone> Collection<T> {
    /// Creates a new [`Collection`] in [`Collection::Init`] state
    pub(crate) fn new() -> Self {
        Self::Init(HashMap::new())
    }

    /// Turns the [`Collection`] into [`Collection::Runtime`] state
    #[inline]
    pub(crate) fn into_runtime(self) -> Self {
        if let Self::Init(map) = self {
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
            Self::Runtime(lock) => lock.read().await.get(key).cloned(),
        }
    }

    /// Inserts a key-value pair into this [`Collection`] when it in [`Collection::Runtime`] state.
    ///
    /// For the [`Collection::Init`] state - use the `as_mut().insert()` method.
    #[inline]
    pub(crate) async fn insert(&self, key: String, value: T) -> Result<(), Error> {
        match self {
            Self::Init(_) => {
                return Err(Error::new(
                    ErrorCode::InternalError,
                    "Attempt to insert a value during runtime when collection is in the init state",
                ));
            }
            Self::Runtime(lock) => lock.write().await.insert(key, value),
        };
        Ok(())
    }

    /// Removes an element from this [`Collection`] by a key when it in [`Collection::Runtime`] state.
    ///
    /// For the [`Collection::Init`] state - use the `as_mut().remove()` method.
    #[inline]
    pub(crate) async fn remove(&self, key: &str) -> Result<Option<T>, Error> {
        let value = match self {
            Self::Init(_) => {
                return Err(Error::new(
                    ErrorCode::InternalError,
                    "Attempt to remove a value during runtime when collection is in the init state",
                ));
            }
            Self::Runtime(lock) => lock.write().await.remove(key),
        };
        Ok(value)
    }

    /// Return a list of values
    #[inline]
    pub(crate) async fn values(&self) -> Vec<T> {
        match self {
            Self::Init(map) => map.values().cloned().collect(),
            Self::Runtime(lock) => lock.read().await.values().cloned().collect(),
        }
    }

    /// Returns a paginated list of values, cloning only the current page.
    #[inline]
    pub(crate) async fn page_values(
        &self,
        cursor: Option<Cursor>,
        page_size: usize,
    ) -> (Vec<T>, Option<Cursor>) {
        match self {
            Self::Init(map) => Self::collect_page(map.values(), cursor, page_size),
            Self::Runtime(lock) => {
                let guard = lock.read().await;
                Self::collect_page(guard.values(), cursor, page_size)
            }
        }
    }

    #[inline]
    fn collect_page<'a>(
        iter: impl Iterator<Item = &'a T>,
        cursor: Option<Cursor>,
        page_size: usize,
    ) -> (Vec<T>, Option<Cursor>)
    where
        T: 'a,
    {
        let start = *cursor.unwrap_or_default();
        let mut iter = iter.skip(start);
        let mut items = Vec::with_capacity(page_size);

        for item in iter.by_ref().take(page_size) {
            items.push(item.clone());
        }

        let next_cursor = iter.next().map(|_| Cursor(start + items.len()));

        (items, next_cursor)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn page_values_returns_only_requested_page() {
        let mut collection = Collection::new();
        collection.as_mut().insert("a".to_string(), 1);
        collection.as_mut().insert("b".to_string(), 2);
        collection.as_mut().insert("c".to_string(), 3);

        let (items, next_cursor) = collection.page_values(None, 2).await;

        assert_eq!(items.len(), 2);
        assert_eq!(next_cursor, Some(Cursor(2)));
    }

    #[tokio::test]
    async fn page_values_returns_empty_page_past_end() {
        let mut collection = Collection::new();
        collection.as_mut().insert("a".to_string(), 1);

        let (items, next_cursor) = collection.page_values(Some(Cursor(5)), 2).await;

        assert!(items.is_empty());
        assert_eq!(next_cursor, None);
    }
}
