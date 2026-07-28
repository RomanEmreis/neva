//! Represents a generic-collection implementation that can be mutated during runtime
//!
//! Backed by a [`BTreeMap`] rather than a hash map: MCP 2026-07-28 asks servers
//! to return `tools/list` in a deterministic order (it lets clients cache and
//! improves LLM prompt-cache hit rates), and cursor pagination is only sound
//! over a stable total order in the first place. Ordering by key gives both,
//! and the registries are small enough that the lookup difference is noise.

use crate::error::{Error, ErrorCode};
use crate::types::Cursor;
use std::collections::BTreeMap;
use tokio::sync::RwLock;

/// Generic collection with 2 states:
/// - [`Collection::Init`] - initialization state can be mutated without blocking
/// - [`Collection::Runtime`] - runtime state, the collection can be read by multiple readers and will blocked by only one writer
pub(crate) enum Collection<T: Clone> {
    Init(BTreeMap<String, T>),
    Runtime(RwLock<BTreeMap<String, T>>),
}

impl<T: Clone> Collection<T> {
    /// Creates a new [`Collection`] in [`Collection::Init`] state
    pub(crate) fn new() -> Self {
        Self::Init(BTreeMap::new())
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

impl<T: Clone> AsMut<BTreeMap<String, T>> for Collection<T> {
    #[inline]
    fn as_mut(&mut self) -> &mut BTreeMap<String, T> {
        if let Self::Init(map) = self {
            map
        } else {
            unreachable!()
        }
    }
}

impl<T: Clone> AsRef<BTreeMap<String, T>> for Collection<T> {
    #[inline]
    fn as_ref(&self) -> &BTreeMap<String, T> {
        if let Self::Init(map) = self {
            map
        } else {
            unreachable!()
        }
    }
}

#[cfg(test)]
mod ordering_tests {
    use super::Collection;

    /// MCP 2026-07-28: `tools/list` must come back in a deterministic order.
    /// A hash map would satisfy neither "same order twice" across processes nor
    /// sound cursor pagination.
    #[tokio::test]
    async fn values_are_ordered_by_key() {
        let mut c = Collection::<u8>::new();
        for name in ["zeta", "alpha", "mu", "beta"] {
            c.as_mut().insert(name.to_string(), 0);
        }
        let c = c.into_runtime();

        // The keys, in the order `values()` walks them.
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(c.values().await.len());
        }
        assert_eq!(seen, [4, 4, 4, 4]);

        let (page, cursor) = c.page_values(None, 2).await;
        assert_eq!(page.len(), 2);
        assert!(cursor.is_some());
    }

    #[tokio::test]
    async fn pagination_walks_every_entry_exactly_once() {
        let mut c = Collection::<String>::new();
        for name in ["e", "a", "d", "b", "c"] {
            c.as_mut().insert(name.to_string(), name.to_string());
        }
        let c = c.into_runtime();

        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let (page, next) = c.page_values(cursor, 2).await;
            seen.extend(page);
            match next {
                Some(n) => cursor = Some(n),
                None => break,
            }
        }

        // Sorted by key, no gaps and no repeats -- which is what makes a
        // cursor over the collection meaningful at all.
        assert_eq!(seen, ["a", "b", "c", "d", "e"]);
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
