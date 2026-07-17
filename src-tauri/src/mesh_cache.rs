//! LRU mesh cache — holds MESH1 bytes keyed by `(BodyId, Lod, generation)`.
//!
//! The `generation` pins a mesh to the snapshot that produced it (plan "Mesh
//! transfer": LRU `MeshCache` keyed `(BodyId, Lod, generation)`), so a stale
//! mesh is never returned for a newer snapshot. Bytes are held behind an `Arc`
//! so a `get_mesh` command hands the webview a zero-copy `tauri::ipc::Response`
//! without re-cloning the blob.
//!
//! The bytes are **opaque** to Rust (MESH1 travels verbatim end-to-end,
//! Invariant 5) — the cache never parses or re-encodes them.

use std::collections::HashMap;
use std::sync::Arc;

use onecad_core::regen::MeshKey;

/// Default entry capacity. Meshes are pull-fetched per visible body/LOD; a few
/// dozen live entries covers a working document without unbounded growth.
pub const DEFAULT_CAPACITY: usize = 64;

/// A bounded LRU cache of MESH1 blobs.
///
/// Eviction is strict LRU: `get` and `put` mark a key most-recently-used; when
/// the entry count exceeds `capacity` the least-recently-used key is dropped.
#[derive(Debug)]
pub struct MeshCache {
    capacity: usize,
    map: HashMap<MeshKey, Arc<Vec<u8>>>,
    /// Keys in ascending recency (front = LRU, back = MRU).
    order: Vec<MeshKey>,
}

impl MeshCache {
    /// A cache with [`DEFAULT_CAPACITY`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// A cache holding at most `capacity` entries (minimum 1).
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            map: HashMap::new(),
            order: Vec::new(),
        }
    }

    /// Number of cached entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// True iff nothing is cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Whether `key` is currently cached (does NOT bump recency).
    #[must_use]
    pub fn contains(&self, key: &MeshKey) -> bool {
        self.map.contains_key(key)
    }

    /// Fetches a blob, marking it most-recently-used. `None` on a miss.
    pub fn get(&mut self, key: &MeshKey) -> Option<Arc<Vec<u8>>> {
        let hit = self.map.get(key).cloned();
        if hit.is_some() {
            self.touch(key);
        }
        hit
    }

    /// Inserts (or replaces) a blob, marking it most-recently-used and evicting
    /// the least-recently-used entry if the cache is over capacity.
    pub fn put(&mut self, key: MeshKey, bytes: Arc<Vec<u8>>) {
        if self.map.insert(key, bytes).is_none() {
            self.order.push(key);
        } else {
            self.touch(&key);
        }
        while self.map.len() > self.capacity {
            if self.order.is_empty() {
                break;
            }
            let evicted = self.order.remove(0);
            self.map.remove(&evicted);
        }
    }

    /// Drops every entry (document close).
    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    /// Moves `key` to the MRU end of the recency order.
    fn touch(&mut self, key: &MeshKey) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            let k = self.order.remove(pos);
            self.order.push(k);
        }
    }
}

impl Default for MeshCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onecad_core::ids::BodyId;
    use onecad_core::regen::Lod;
    use uuid::Uuid;

    fn key(n: u128, gen: u64) -> MeshKey {
        MeshKey {
            body: BodyId(Uuid::from_u128(n)),
            lod: Lod::Coarse,
            generation: gen,
        }
    }

    #[test]
    fn miss_then_hit() {
        let mut c = MeshCache::new();
        let k = key(1, 1);
        assert!(c.get(&k).is_none(), "cold miss");
        let bytes = Arc::new(vec![1u8, 2, 3]);
        c.put(k, bytes.clone());
        let got = c.get(&k).expect("hit");
        assert_eq!(*got, vec![1, 2, 3]);
        assert!(
            Arc::ptr_eq(&got, &bytes),
            "same Arc handed back (zero-copy)"
        );
    }

    #[test]
    fn distinct_generation_is_a_distinct_entry() {
        let mut c = MeshCache::new();
        c.put(key(1, 1), Arc::new(vec![1]));
        c.put(key(1, 2), Arc::new(vec![2]));
        assert_eq!(c.len(), 2, "generation is part of the key");
        assert_eq!(*c.get(&key(1, 1)).unwrap(), vec![1]);
        assert_eq!(*c.get(&key(1, 2)).unwrap(), vec![2]);
    }

    #[test]
    fn evicts_least_recently_used() {
        let mut c = MeshCache::with_capacity(2);
        c.put(key(1, 1), Arc::new(vec![1]));
        c.put(key(2, 1), Arc::new(vec![2]));
        // Touch key 1 so key 2 becomes the LRU victim.
        let _ = c.get(&key(1, 1));
        c.put(key(3, 1), Arc::new(vec![3]));
        assert_eq!(c.len(), 2);
        assert!(c.contains(&key(1, 1)), "recently used survives");
        assert!(!c.contains(&key(2, 1)), "LRU evicted");
        assert!(c.contains(&key(3, 1)));
    }

    #[test]
    fn clear_drops_everything() {
        let mut c = MeshCache::new();
        c.put(key(1, 1), Arc::new(vec![1]));
        c.clear();
        assert!(c.is_empty());
    }
}
