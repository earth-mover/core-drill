use std::collections::HashMap;

use super::{DataStore, LoadState};
use super::types::TreeNodeType;

/// Aggregated storage stats computed from the current tree + chunk stats cache.
#[derive(Default)]
pub(crate) struct StorageStats {
    pub total_arrays: usize,
    pub total_groups: usize,
    pub total_written: u64,
    pub empty_arrays: usize,
    pub filled_arrays: usize,
    pub known_native: usize,
    pub known_inline: usize,
    pub known_virtual: usize,
    pub native_bytes: u64,
    pub inline_bytes: u64,
    pub virtual_bytes: u64,
    pub stats_loaded: usize,
    pub virtual_prefixes: HashMap<String, usize>,
}

impl StorageStats {
    pub fn from_store(store: &DataStore) -> Self {
        let mut s = Self {
            total_arrays: 0,
            total_groups: 0,
            total_written: 0,
            empty_arrays: 0,
            filled_arrays: 0,
            known_native: 0,
            known_inline: 0,
            known_virtual: 0,
            native_bytes: 0,
            inline_bytes: 0,
            virtual_bytes: 0,
            stats_loaded: 0,
            virtual_prefixes: HashMap::new(),
        };

        for state in store.node_children.values() {
            if let LoadState::Loaded(nodes) = state {
                for node in nodes {
                    match &node.node_type {
                        TreeNodeType::Group => s.total_groups += 1,
                        TreeNodeType::Array(summary) => {
                            s.total_arrays += 1;
                            if let Some(tc) = summary.total_chunks {
                                s.total_written += tc;
                                if tc == 0 {
                                    s.empty_arrays += 1;
                                } else {
                                    s.filled_arrays += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        for ((_, _), state) in &store.chunk_stats {
            if let LoadState::Loaded(stats) = state {
                s.stats_loaded += 1;
                s.known_native += stats.native_count;
                s.known_inline += stats.inline_count;
                s.known_virtual += stats.virtual_count;
                s.native_bytes += stats.native_total_bytes;
                s.inline_bytes += stats.inline_total_bytes;
                s.virtual_bytes += stats.virtual_total_bytes;
                for (prefix, count) in &stats.virtual_prefixes {
                    *s.virtual_prefixes.entry(prefix.clone()).or_insert(0) += count;
                }
            }
        }

        s
    }

    pub fn total_bytes(&self) -> u64 {
        self.native_bytes + self.inline_bytes + self.virtual_bytes
    }

    pub fn stored_bytes(&self) -> u64 {
        self.native_bytes + self.inline_bytes
    }

    pub fn breakdown_parts(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.known_native > 0 {
            parts.push(format!("{} native", self.known_native));
        }
        if self.known_inline > 0 {
            parts.push(format!("{} inline", self.known_inline));
        }
        if self.known_virtual > 0 {
            parts.push(format!("{} virtual", self.known_virtual));
        }
        parts
    }
}
