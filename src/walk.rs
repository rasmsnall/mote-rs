use std::path::PathBuf;

use jwalk::WalkDir;
use serde::{Deserialize, Serialize};

/// A single regular file discovered during a scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub size: u64,
}

/// Walk `root` in parallel, returning every regular file.
///
/// Symlinks are not followed: a link farm would otherwise produce phantom
/// duplicates, and following one out of `root` would let a scan escape the
/// mounted volume.
pub fn scan(root: &std::path::Path) -> Vec<Entry> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let size = e.metadata().ok()?.len();
            Some(Entry {
                path: e.path(),
                size,
            })
        })
        .collect()
}
