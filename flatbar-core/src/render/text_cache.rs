//! Bounded LRU cache for shaped glyph runs and text measurements.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Cache key for shaped text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextCacheKey {
    pub text: String,
    pub font_family: String,
    pub font_size_bits: u32,
    pub scale_bits: u64,
}

impl TextCacheKey {
    pub fn new(text: &str, font_family: &str, font_size: f32, scale: f64) -> Self {
        Self {
            text: text.to_string(),
            font_family: font_family.to_string(),
            font_size_bits: font_size.to_bits(),
            scale_bits: scale.to_bits(),
        }
    }
}

/// A cached shaped glyph with its relative offset and alpha coverage bitmap.
#[derive(Debug, Clone)]
pub struct CachedGlyph {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub alpha_mask: Vec<u8>,
}

/// A cached shaped run.
#[derive(Debug, Clone)]
pub struct ShapedRun {
    pub width: u32,
    pub height: u32,
    pub glyphs: Vec<CachedGlyph>,
}

/// Bounded LRU shaping cache.
pub struct ShapingCache {
    max_entries: usize,
    map: Mutex<HashMap<TextCacheKey, ShapedRun>>,
    order: Mutex<VecDeque<TextCacheKey>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl Default for ShapingCache {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl ShapingCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(16),
            map: Mutex::new(HashMap::new()),
            order: Mutex::new(VecDeque::new()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Look up a cached shaped run.
    pub fn get(&self, key: &TextCacheKey) -> Option<ShapedRun> {
        let map = self.map.lock().unwrap();
        if let Some(run) = map.get(key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            Some(run.clone())
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Insert a newly shaped run into the cache, evicting the oldest if full.
    pub fn insert(&self, key: TextCacheKey, run: ShapedRun) {
        let mut map = self.map.lock().unwrap();
        let mut order = self.order.lock().unwrap();

        if let Some(existing) = map.get_mut(&key) {
            *existing = run;
            return;
        }

        if map.len() >= self.max_entries {
            if let Some(oldest) = order.pop_front() {
                map.remove(&oldest);
            }
        }

        order.push_back(key.clone());
        map.insert(key, run);
    }

    /// Clear all cached shaped runs.
    pub fn clear(&self) {
        let mut map = self.map.lock().unwrap();
        let mut order = self.order.lock().unwrap();
        map.clear();
        order.clear();
    }

    /// Get cache statistics: `(hits, misses, current_entries)`.
    pub fn stats(&self) -> (u64, u64, usize) {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let entries = self.map.lock().unwrap().len();
        (hits, misses, entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shaping_cache_lru() {
        let cache = ShapingCache::new(2);
        let k1 = TextCacheKey::new("1", "sans", 13.0, 1.0);
        let k2 = TextCacheKey::new("2", "sans", 13.0, 1.0);
        let k3 = TextCacheKey::new("3", "sans", 13.0, 1.0);

        let run = ShapedRun {
            width: 10,
            height: 10,
            glyphs: vec![],
        };

        cache.insert(k1.clone(), run.clone());
        cache.insert(k2.clone(), run.clone());

        assert!(cache.get(&k1).is_some());
        assert_eq!(cache.stats().0, 1); // 1 hit

        // Insert k3 -> should evict k1 or oldest
        cache.insert(k3.clone(), run.clone());
        assert!(cache.get(&k3).is_some());
    }
}
