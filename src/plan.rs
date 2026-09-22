use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::dedupe::Group;

/// A reviewable, re-runnable description of what `apply` would do.
///
/// Scanning never touches the filesystem; it only emits one of these. That
/// keeps dry-run the default and makes the destructive phase an explicit,
/// separate step operating on a file a human can read first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub root: PathBuf,
    pub scanned: usize,
    pub actions: Vec<Action>,
    pub duplicates: Vec<Group>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Action {
    /// Move a file to its routed destination.
    Move { from: PathBuf, to: PathBuf },
    /// Set a duplicate aside without destroying it.
    Quarantine { from: PathBuf, to: PathBuf },
}

impl Plan {
    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn from_json(s: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(s)?)
    }

    /// Bytes that would be reclaimed by resolving every duplicate group.
    pub fn reclaimable(&self) -> u64 {
        self.duplicates
            .iter()
            .map(|g| g.size * (g.paths.len() as u64 - 1))
            .sum()
    }
}
