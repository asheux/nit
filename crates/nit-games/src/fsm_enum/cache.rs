//! Memoisation caches shared across canonicalisation and behaviour grouping.
//!
//! The two caches are keyed differently — `(states, actions)` for the
//! canonical-index list and `(states, actions, mode)` for behaviour
//! representatives — so they live behind separate `Mutex<HashMap<_>>`s
//! while sharing the `clone_cached_vec_result` accessor.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Mutex, OnceLock};

use crate::config::FsmGroupingMode;

pub(super) type CanonicalFsmCache = HashMap<(usize, usize), Result<Vec<u64>, String>>;
pub(super) type BehaviorRepCache =
    HashMap<(usize, usize, FsmGroupingMode), Result<Vec<u64>, String>>;

pub(super) fn canonical_fsm_cache() -> &'static Mutex<CanonicalFsmCache> {
    static CACHE: OnceLock<Mutex<CanonicalFsmCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn behavior_rep_cache() -> &'static Mutex<BehaviorRepCache> {
    static CACHE: OnceLock<Mutex<BehaviorRepCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn clone_cached_vec_result<K>(
    cache: &Mutex<HashMap<K, Result<Vec<u64>, String>>>,
    key: K,
    compute: impl FnOnce(&K) -> Result<Vec<u64>, String>,
) -> Result<Vec<u64>, String>
where
    K: Eq + Hash + Clone,
{
    if let Some(cached) = cache
        .lock()
        .expect("fsm cache lock poisoned")
        .get(&key)
        .cloned()
    {
        return cached;
    }
    let computed = compute(&key);
    cache
        .lock()
        .expect("fsm cache lock poisoned")
        .insert(key, computed.clone());
    computed
}

pub(super) fn insert_min_index(map: &mut HashMap<Vec<u16>, u64>, key: Vec<u16>, idx: u64) {
    map.entry(key)
        .and_modify(|existing| *existing = (*existing).min(idx))
        .or_insert(idx);
}

pub(super) fn merge_min_index_maps(
    left: &mut HashMap<Vec<u16>, u64>,
    right: HashMap<Vec<u16>, u64>,
) {
    for (key, idx) in right {
        insert_min_index(left, key, idx);
    }
}
