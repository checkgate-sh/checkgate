use crate::evaluator::Flag;
use dashmap::DashMap;
use std::sync::Arc;

/// The central in-memory store for feature flags.
/// Flags are stored behind Arc so get_flag is a single atomic increment (no heap allocation).
#[derive(Clone, Default)]
pub struct FlagStore {
    flags: Arc<DashMap<String, Arc<Flag>>>,
}

impl FlagStore {
    pub fn new() -> Self {
        Self {
            flags: Arc::new(DashMap::new()),
        }
    }

    pub fn upsert_flag(&self, flag: Flag) {
        self.flags.insert(flag.key.clone(), Arc::new(flag));
    }

    /// Returns a cheap Arc clone — no Flag data is copied.
    pub fn get_flag(&self, key: &str) -> Option<Arc<Flag>> {
        self.flags.get(key).map(|r| Arc::clone(r.value()))
    }

    pub fn delete_flag(&self, key: &str) {
        self.flags.remove(key);
    }

    /// Returns Arc clones of all flags — individual Flag data is not copied.
    pub fn list_flags(&self) -> Vec<Arc<Flag>> {
        self.flags.iter().map(|r| Arc::clone(r.value())).collect()
    }

    pub fn clear(&self) {
        self.flags.clear();
    }

    /// A new store holding the same flags, independent of this one: changes
    /// to either are not seen by the other. The flags themselves are shared
    /// (`Arc` clones), so this copies one pointer per flag, never flag data.
    ///
    /// Lets a binding build the next version of its flag set beside the one
    /// being evaluated and swap it in whole (copy-on-write), so a concurrent
    /// evaluation sees either the old set or the new one, never a mix.
    #[must_use]
    pub fn independent_copy(&self) -> Self {
        Self {
            flags: Arc::new((*self.flags).clone()),
        }
    }

    /// Number of flags held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.flags.len()
    }

    /// Whether the store holds no flags.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.flags.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluator::{FlagType, FlagValue};

    fn mk(key: &str, enabled: bool) -> Flag {
        Flag {
            key: key.into(),
            is_enabled: enabled,
            rollout_percentage: None,
            description: None,
            rules: vec![],
            flag_type: FlagType::Boolean,
            default_value: None,
            disabled_value: None,
            variants: vec![],
            prerequisites: vec![],
        }
    }

    #[test]
    fn upsert_then_get_returns_the_flag() {
        let store = FlagStore::new();
        assert!(store.get_flag("a").is_none());
        store.upsert_flag(mk("a", true));
        let got = store.get_flag("a").expect("flag present");
        assert_eq!(got.key, "a");
        assert!(got.is_enabled);
    }

    #[test]
    fn upsert_same_key_overwrites() {
        let store = FlagStore::new();
        store.upsert_flag(mk("a", true));
        store.upsert_flag(mk("a", false));
        assert!(!store.get_flag("a").unwrap().is_enabled);
        // Still exactly one entry for the key.
        assert_eq!(store.list_flags().len(), 1);
    }

    #[test]
    fn get_missing_is_none() {
        let store = FlagStore::new();
        assert!(store.get_flag("nope").is_none());
    }

    #[test]
    fn delete_removes_only_the_target() {
        let store = FlagStore::new();
        store.upsert_flag(mk("a", true));
        store.upsert_flag(mk("b", true));
        store.delete_flag("a");
        assert!(store.get_flag("a").is_none());
        assert!(store.get_flag("b").is_some());
        assert_eq!(store.list_flags().len(), 1);
    }

    #[test]
    fn delete_missing_is_a_noop() {
        let store = FlagStore::new();
        store.upsert_flag(mk("a", true));
        store.delete_flag("ghost"); // must not panic or affect others
        assert_eq!(store.list_flags().len(), 1);
    }

    #[test]
    fn list_returns_all_and_clear_empties() {
        let store = FlagStore::new();
        for k in ["a", "b", "c"] {
            store.upsert_flag(mk(k, true));
        }
        let mut keys: Vec<String> = store.list_flags().iter().map(|f| f.key.clone()).collect();
        keys.sort();
        assert_eq!(keys, vec!["a", "b", "c"]);
        store.clear();
        assert_eq!(store.list_flags().len(), 0);
        assert!(store.get_flag("a").is_none());
    }

    #[test]
    fn clone_shares_the_same_underlying_map() {
        // FlagStore is Clone; clones must share state (Arc), not snapshot it —
        // the server relies on this to update the store from its Redis subscriber.
        let store = FlagStore::new();
        let clone = store.clone();
        store.upsert_flag(mk("shared", true));
        assert!(
            clone.get_flag("shared").is_some(),
            "clone should see the write"
        );
        clone.delete_flag("shared");
        assert!(
            store.get_flag("shared").is_none(),
            "original should see the delete"
        );
    }

    #[test]
    fn independent_copy_shares_flags_but_not_changes() {
        let store = FlagStore::new();
        store.upsert_flag(mk("a", true));
        let copy = store.independent_copy();
        assert_eq!(copy.len(), 1);
        assert!(Arc::ptr_eq(
            &store.get_flag("a").unwrap(),
            &copy.get_flag("a").unwrap()
        ));
        copy.upsert_flag(mk("b", true));
        copy.delete_flag("a");
        assert!(store.get_flag("a").is_some(), "original keeps its flag");
        assert!(
            store.get_flag("b").is_none(),
            "original never sees the copy's write"
        );
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
        assert!(FlagStore::new().is_empty());
    }

    #[test]
    fn get_flag_returns_cheap_arc_clone() {
        let store = FlagStore::new();
        let mut f = mk("v", true);
        f.default_value = Some(FlagValue::Str("x".into()));
        store.upsert_flag(f);
        let a = store.get_flag("v").unwrap();
        let b = store.get_flag("v").unwrap();
        // Both handles point at the same Arc allocation.
        assert!(Arc::ptr_eq(&a, &b));
    }
}
