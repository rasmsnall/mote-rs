use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::dedupe::Group;
use crate::ident::Format;

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
    /// Files no rule claimed. Left where they are; listed so the gap in the
    /// rules is visible instead of silent.
    #[serde(default)]
    pub unroutable: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Action {
    /// Move a file to its routed destination.
    Move { from: PathBuf, to: PathBuf },
    /// Set a file aside without destroying it.
    Quarantine {
        from: PathBuf,
        to: PathBuf,
        reason: String,
    },
    /// Expand an archive, filing each member by what it turned out to be.
    ///
    /// The members are resolved at scan time, so the plan states every path
    /// that will be written before anything is. The archive itself is left
    /// where it is: deleting the only copy of a container on the strength of
    /// an extraction that has not been checked yet is not recoverable.
    Extract {
        from: PathBuf,
        format: Format,
        members: Vec<PlannedMember>,
    },
}

/// One file inside a planned archive extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedMember {
    /// Path inside the archive, sanitised at scan time.
    pub name: PathBuf,
    /// Where it lands.
    pub to: PathBuf,
    /// Size the archive's header claims, and the cap enforced while writing.
    pub size: u64,
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

    /// Bytes `apply` would write out of archives.
    pub fn extracted_bytes(&self) -> u64 {
        self.actions
            .iter()
            .filter_map(|a| match a {
                Action::Extract { members, .. } => {
                    Some(members.iter().map(|m| m.size).sum::<u64>())
                }
                _ => None,
            })
            .sum()
    }

    /// A one-line-per-kind tally for the scan summary.
    pub fn tally(&self) -> (usize, usize, usize) {
        let mut moves = 0;
        let mut quarantines = 0;
        let mut extracts = 0;
        for action in &self.actions {
            match action {
                Action::Move { .. } => moves += 1,
                Action::Quarantine { .. } => quarantines += 1,
                Action::Extract { .. } => extracts += 1,
            }
        }
        (moves, quarantines, extracts)
    }
}
