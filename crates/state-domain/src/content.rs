//! Composable loading of immutable content, independent of catalog storage.
use crate::{Error, ErrorCode, Result, identity::hash};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

/// Parsed SHA-256 address of immutable content.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContentHash(String);
impl ContentHash {
    /// Parse a canonical lowercase SHA-256 address.
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(Error::new(
                ErrorCode::CorruptStore,
                "invalid content digest",
            ));
        }
        Ok(Self(value))
    }
    /// Address used for storage lookup.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self(hash(bytes))
    }
}
impl TryFrom<String> for ContentHash {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        Self::parse(value)
    }
}
impl From<ContentHash> for String {
    fn from(value: ContentHash) -> Self {
        value.0
    }
}

/// Storage port supplying bytes from the current transaction's view.
pub trait ContentSource {
    /// Read an object; missing objects and I/O errors must be reported.
    fn read(&self, address: &ContentHash) -> Result<Vec<u8>>;
}

/// Load and verify immutable bytes. Implementations must reject digest mismatches.
/// Caches may retain only committed objects supplied by the storage adapter.
pub trait ContentLoader: Send + Sync {
    /// Return verified content without changing the transaction or its catalog.
    fn load(&self, source: &dyn ContentSource, address: &ContentHash) -> Result<Arc<[u8]>>;
}

/// Load directly from storage and verify the digest on every read.
#[derive(Default)]
pub struct DirectLoader;
impl ContentLoader for DirectLoader {
    fn load(&self, source: &dyn ContentSource, address: &ContentHash) -> Result<Arc<[u8]>> {
        let bytes = source.read(address)?;
        if ContentHash::of(&bytes) != *address {
            return Err(Error::new(
                ErrorCode::CorruptStore,
                "content digest differs from its bytes",
            ));
        }
        Ok(bytes.into())
    }
}

/// Bounded FIFO cache decorating any content loader. Oversized objects bypass
/// the cache; failed loads are never cached. Eviction drops cache ownership only,
/// so active invocations keep their immutable content alive.
pub struct CachingLoader<L> {
    inner: L,
    capacity: usize,
    cache: Mutex<Cache>,
}
#[derive(Default)]
struct Cache {
    entries: BTreeMap<ContentHash, Arc<[u8]>>,
    order: VecDeque<ContentHash>,
    bytes: usize,
}
impl<L: ContentLoader> CachingLoader<L> {
    /// Wrap a loader with a byte budget. Zero disables retention.
    pub fn new(inner: L, capacity: usize) -> Self {
        Self {
            inner,
            capacity,
            cache: Mutex::new(Cache::default()),
        }
    }
}
impl<L: ContentLoader> ContentLoader for CachingLoader<L> {
    fn load(&self, source: &dyn ContentSource, address: &ContentHash) -> Result<Arc<[u8]>> {
        let lock = || {
            self.cache
                .lock()
                .map_err(|_| Error::new(ErrorCode::CorruptStore, "content cache lock is poisoned"))
        };
        if let Some(bytes) = lock()?.entries.get(address) {
            return Ok(Arc::clone(bytes));
        }
        // No cache lock across storage I/O. Concurrent misses can safely load
        // the same immutable object; the second insertion reuses the first.
        let bytes = self.inner.load(source, address)?;
        if bytes.len() > self.capacity || self.capacity == 0 {
            return Ok(bytes);
        }
        let mut cache = lock()?;
        if let Some(existing) = cache.entries.get(address) {
            return Ok(Arc::clone(existing));
        }
        while cache.bytes > self.capacity - bytes.len() {
            let oldest = cache.order.pop_front().ok_or_else(|| {
                Error::new(
                    ErrorCode::CorruptStore,
                    "inconsistent content cache accounting",
                )
            })?;
            if let Some(removed) = cache.entries.remove(&oldest) {
                cache.bytes -= removed.len();
            }
        }
        cache.bytes += bytes.len();
        cache.order.push_back(address.clone());
        cache.entries.insert(address.clone(), Arc::clone(&bytes));
        Ok(bytes)
    }
}

/// Transaction-scoped catalog adapter; never exposes pending object writes to
/// the process cache. Publication places those writes in the catalog first.
pub(crate) struct CatalogContent<'a>(pub &'a rusqlite::Connection);
impl ContentSource for CatalogContent<'_> {
    fn read(&self, address: &ContentHash) -> Result<Vec<u8>> {
        use rusqlite::OptionalExtension;
        self.0
            .prepare_cached("SELECT bytes FROM objects WHERE hash=?")?
            .query_row([address.as_str()], |row| row.get(0))
            .optional()?
            .ok_or_else(|| Error::new(ErrorCode::CorruptStore, "content object is missing"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Source {
        bytes: Vec<u8>,
        reads: AtomicUsize,
    }
    impl Source {
        fn new(bytes: &[u8]) -> Self {
            Self {
                bytes: bytes.to_vec(),
                reads: AtomicUsize::new(0),
            }
        }
    }
    impl ContentSource for Source {
        fn read(&self, _: &ContentHash) -> Result<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(self.bytes.clone())
        }
    }
    #[test]
    fn direct_and_cached_loaders_verify_content_and_failures_are_not_cached() -> Result<()> {
        let source = Source::new(b"hello");
        let address = ContentHash::of(b"hello");
        let direct = DirectLoader;
        direct.load(&source, &address)?;
        direct.load(&source, &address)?;
        assert_eq!(source.reads.load(Ordering::SeqCst), 2);
        let cache = CachingLoader::new(direct, 16);
        let first = cache.load(&source, &address)?;
        let second = cache.load(&source, &address)?;
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(source.reads.load(Ordering::SeqCst), 3);
        let wrong = ContentHash::of(b"other");
        for _ in 0..2 {
            assert_eq!(
                cache.load(&source, &wrong).err().map(|e| e.code),
                Some(ErrorCode::CorruptStore)
            );
        }
        assert_eq!(source.reads.load(Ordering::SeqCst), 5);
        Ok(())
    }
    #[test]
    fn cache_evicts_within_budget_without_invalidating_active_content() -> Result<()> {
        let cache = CachingLoader::new(DirectLoader, 4);
        let a = Source::new(b"aaa");
        let b = Source::new(b"bbb");
        let first = cache.load(&a, &ContentHash::of(b"aaa"))?;
        cache.load(&b, &ContentHash::of(b"bbb"))?;
        assert_eq!(&*first, b"aaa");
        cache.load(&a, &ContentHash::of(b"aaa"))?;
        assert_eq!(a.reads.load(Ordering::SeqCst), 2);
        let big = Source::new(b"too large");
        for _ in 0..2 {
            cache.load(&big, &ContentHash::of(b"too large"))?;
        }
        assert_eq!(big.reads.load(Ordering::SeqCst), 2);
        assert!(
            cache
                .cache
                .lock()
                .map_err(|_| Error::invalid("test cache lock poisoned"))?
                .bytes
                <= 4
        );
        Ok(())
    }
}
