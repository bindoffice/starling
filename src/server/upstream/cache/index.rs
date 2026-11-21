use super::*;

const CACHE_INDEX_ENTRY_BYTES: u64 = 128;

#[derive(Debug, Default)]
struct CacheIndexEntry {
    uses: u32,
}

#[derive(Debug)]
pub(super) struct CacheIndex {
    limit_bytes: u64,
    used_bytes: u64,
    entries: HashMap<String, CacheIndexEntry>,
}

impl CacheIndex {
    pub(super) fn new(limit_bytes: usize) -> Self {
        Self {
            limit_bytes: u64::try_from(limit_bytes).unwrap_or(u64::MAX),
            used_bytes: 0,
            entries: HashMap::new(),
        }
    }

    pub(super) fn note_use(&mut self, hash: &str, min_uses: usize) -> bool {
        if let Some(entry) = self.entries.get_mut(hash) {
            entry.uses = entry.uses.saturating_add(1);
            return entry.uses as usize >= min_uses;
        }
        if !self.can_insert() {
            return false;
        }
        self.entries
            .insert(hash.to_string(), CacheIndexEntry { uses: 1 });
        self.used_bytes = self.used_bytes.saturating_add(CACHE_INDEX_ENTRY_BYTES);
        min_uses <= 1
    }

    pub(super) fn load(&mut self, hash: &str) -> bool {
        if let Some(entry) = self.entries.get_mut(hash) {
            entry.uses = entry.uses.max(1);
            return true;
        }
        if !self.can_insert() {
            return false;
        }
        self.entries
            .insert(hash.to_string(), CacheIndexEntry { uses: 1 });
        self.used_bytes = self.used_bytes.saturating_add(CACHE_INDEX_ENTRY_BYTES);
        true
    }

    pub(super) fn remove(&mut self, hash: &str) {
        if self.entries.remove(hash).is_some() {
            self.used_bytes = self.used_bytes.saturating_sub(CACHE_INDEX_ENTRY_BYTES);
        }
    }

    fn can_insert(&self) -> bool {
        self.used_bytes.saturating_add(CACHE_INDEX_ENTRY_BYTES) <= self.limit_bytes
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_keys_zone_capacity_and_preserves_use_counts() {
        let mut index = CacheIndex::new((CACHE_INDEX_ENTRY_BYTES * 2) as usize);
        assert!(!index.note_use("a", 2));
        assert!(index.note_use("a", 2));
        assert!(index.load("b"));
        assert!(!index.note_use("c", 1));
        index.remove("a");
        assert!(index.note_use("c", 1));
        assert_eq!(index.len(), 2);
    }
}
