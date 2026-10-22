//! The page textures on the GPU, kept within a budget: full renders and
//! previews apart, each least-recently-used, and nothing drawn in the
//! last frame is ever evicted. Pure bookkeeping, tested with sizes instead
//! of textures.

use std::collections::HashMap;
use std::rc::Rc;

use crate::engine::{Kind, RenderKey};
use crate::textures::Texture;

/// Bytes of full renders kept (level 0 plus mips): about 100 megapixels.
pub const FULL_BUDGET: usize = 400 << 20;
/// Bytes of previews kept: about 450 pages at 200 × 260.
pub const PREVIEW_BUDGET: usize = 24 << 20;

struct Entry<T> {
    item: Rc<T>,
    bytes: usize,
    /// The frame it was last drawn or inserted in.
    used: u64,
}

/// The textures of one kind under one budget.
pub struct Pool<T> {
    entries: HashMap<RenderKey, Entry<T>>,
    budget: usize,
    bytes: usize,
}

impl<T> Pool<T> {
    pub fn new(budget: usize) -> Self {
        Pool { entries: HashMap::new(), budget, bytes: 0 }
    }

    /// Put `item` in, replacing one under the same key.
    pub fn insert(&mut self, key: RenderKey, item: T, bytes: usize, frame: u64) {
        if let Some(old) = self.entries.insert(key, Entry { item: Rc::new(item), bytes, used: frame }) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
    }

    /// The item under `key`, marked as used in `frame`.
    pub fn get(&mut self, key: &RenderKey, frame: u64) -> Option<Rc<T>> {
        let e = self.entries.get_mut(key)?;
        e.used = frame;
        Some(Rc::clone(&e.item))
    }

    pub fn contains(&self, key: &RenderKey) -> bool {
        self.entries.contains_key(key)
    }

    /// The entries of `doc`'s page `page` with `turns`, by width: the
    /// nearest render while the exact one is not there.
    pub fn nearest(&mut self, key: &RenderKey, frame: u64) -> Option<(u32, Rc<T>)> {
        let best = self
            .entries
            .iter()
            .filter(|(k, _)| k.doc == key.doc && k.page == key.page && k.turns == key.turns && k.kind == key.kind)
            .min_by_key(|(k, _)| {
                // Prefer larger renders (sharper when scaled down), the
                // nearest among them.
                let larger = k.width >= key.width;
                (!larger, k.width.abs_diff(key.width))
            })
            .map(|(k, _)| *k)?;
        let e = self.entries.get_mut(&best)?;
        e.used = frame;
        Some((best.width, Rc::clone(&e.item)))
    }

    /// Drop the least recently used entries over the budget, never those
    /// used in `frame` (the frame just drawn).
    pub fn trim(&mut self, frame: u64) {
        if self.bytes <= self.budget {
            return;
        }
        let mut by_age: Vec<(u64, RenderKey, usize)> =
            self.entries.iter().filter(|(_, e)| e.used < frame).map(|(k, e)| (e.used, *k, e.bytes)).collect();
        by_age.sort_unstable_by_key(|(used, _, _)| *used);
        for (_, key, bytes) in by_age {
            if self.bytes <= self.budget {
                break;
            }
            self.entries.remove(&key);
            self.bytes -= bytes;
        }
    }

    /// Forget the renders of `page` of document `doc` (its content changed).
    pub fn remove_page(&mut self, doc: crate::engine::DocId, page: u32) {
        let gone: Vec<RenderKey> = self.entries.keys().filter(|k| k.doc == doc && k.page == page).copied().collect();
        for k in gone {
            if let Some(e) = self.entries.remove(&k) {
                self.bytes -= e.bytes;
            }
        }
    }

    /// Forget everything of document `doc`.
    pub fn remove_doc(&mut self, doc: crate::engine::DocId) {
        let gone: Vec<RenderKey> = self.entries.keys().filter(|k| k.doc == doc).copied().collect();
        for k in gone {
            if let Some(e) = self.entries.remove(&k) {
                self.bytes -= e.bytes;
            }
        }
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Both pools.
pub struct Textures {
    pub full: Pool<Texture>,
    pub preview: Pool<Texture>,
}

impl Default for Textures {
    fn default() -> Self {
        Textures { full: Pool::new(FULL_BUDGET), preview: Pool::new(PREVIEW_BUDGET) }
    }
}

impl Textures {
    pub fn pool(&mut self, kind: Kind) -> &mut Pool<Texture> {
        match kind {
            Kind::Full => &mut self.full,
            Kind::Preview => &mut self.preview,
        }
    }

    pub fn trim(&mut self, frame: u64) {
        self.full.trim(frame);
        self.preview.trim(frame);
    }

    pub fn remove_doc(&mut self, doc: crate::engine::DocId) {
        self.full.remove_doc(doc);
        self.preview.remove_doc(doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(page: u32, width: u32) -> RenderKey {
        RenderKey { doc: 1, page, width, turns: 0, kind: Kind::Full }
    }

    #[test]
    fn the_pool_evicts_the_oldest_but_never_the_last_frame() {
        let mut pool: Pool<u32> = Pool::new(100);
        pool.insert(key(0, 10), 0, 40, 1);
        pool.insert(key(1, 10), 1, 40, 2);
        pool.insert(key(2, 10), 2, 40, 3);
        assert_eq!(pool.bytes(), 120);
        pool.trim(3);
        assert_eq!(pool.bytes(), 80);
        assert!(!pool.contains(&key(0, 10)));
        // Everything used this frame stays, even over the budget.
        pool.insert(key(3, 10), 3, 40, 4);
        pool.get(&key(1, 10), 4);
        pool.get(&key(2, 10), 4);
        pool.trim(4);
        assert_eq!(pool.len(), 3);
        // The next frame, the oldest of them goes.
        pool.trim(5);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn the_nearest_render_prefers_a_larger_one() {
        let mut pool: Pool<u32> = Pool::new(1000);
        pool.insert(key(0, 500), 0, 1, 1);
        pool.insert(key(0, 1200), 1, 1, 1);
        pool.insert(key(1, 800), 2, 1, 1);
        assert_eq!(pool.nearest(&key(0, 800), 2).map(|(w, _)| w), Some(1200));
        assert_eq!(pool.nearest(&key(0, 2000), 2).map(|(w, _)| w), Some(1200));
        assert_eq!(pool.nearest(&key(0, 300), 2).map(|(w, _)| w), Some(500));
        assert_eq!(pool.nearest(&key(2, 300), 2).map(|(w, _)| w), None);
        pool.remove_doc(1);
        assert_eq!(pool.len(), 0);
        assert_eq!(pool.bytes(), 0);
    }
}
