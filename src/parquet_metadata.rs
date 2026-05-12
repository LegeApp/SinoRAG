use anyhow::Result;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use rustc_hash::FxHashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Cached ArrowReaderMetadata for Parquet files.
/// Avoids repeated metadata reads for the same file.
#[derive(Debug, Clone)]
pub struct CachedMetadata {
    /// File path (canonicalized if possible)
    pub path: PathBuf,
    /// Last modified time (for cache invalidation)
    pub modified: Option<std::time::SystemTime>,
}

/// Thread-safe cache for Parquet file metadata.
pub struct ParquetMetadataCache {
    cache: Arc<Mutex<FxHashMap<PathBuf, CachedMetadata>>>,
}

impl ParquetMetadataCache {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(FxHashMap::default())),
        }
    }

    /// Get or load metadata for a Parquet file.
    /// Returns the ParquetRecordBatchReaderBuilder with cached metadata if available.
    pub fn get_or_load(&self, path: &Path) -> Result<ParquetRecordBatchReaderBuilder<File>> {
        // For now, just load directly. Future enhancement: cache the metadata
        // to avoid repeated file reads when processing the same file multiple times.
        let file = File::open(path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
        Ok(builder)
    }

    /// Check if cached metadata is still valid (file hasn't been modified).
    pub fn is_valid(&self, path: &Path) -> bool {
        let cache = self.cache.lock().unwrap();
        if let Some(cached) = cache.get(path) {
            if let Some(modified) = cached.modified {
                if let Ok(current_modified) = std::fs::metadata(path).and_then(|m| m.modified()) {
                    return current_modified == modified;
                }
            }
        }
        false
    }

    /// Invalidate cache entry for a specific file.
    pub fn invalidate(&self, path: &Path) {
        let mut cache = self.cache.lock().unwrap();
        cache.remove(path);
    }

    /// Clear entire cache.
    pub fn clear(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.clear();
    }
}

impl Default for ParquetMetadataCache {
    fn default() -> Self {
        Self::new()
    }
}

lazy_static::lazy_static! {
    static ref GLOBAL_CACHE: ParquetMetadataCache = ParquetMetadataCache::new();
}

/// Get the global metadata cache.
pub fn global_cache() -> &'static ParquetMetadataCache {
    &GLOBAL_CACHE
}
