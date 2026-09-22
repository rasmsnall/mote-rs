use std::path::PathBuf;

use jwalk::WalkDir;
use serde::{Deserialize, Serialize};

/// A single regular file discovered during a scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub size: u64,
}

/// Walk `root` in parallel, returning every regular file in path order.
///
/// Symlinks are not followed: a link farm would otherwise produce phantom
/// duplicates, and following one out of `root` would let a scan escape the
/// mounted volume.
///
/// The result is sorted. Parallel traversal finishes in whatever order the
/// threads happen to, and every later stage — which duplicate survives, which
/// file wins a contested destination — depends on that order. Sorting here is
/// what makes two scans of an unchanged tree produce the same plan, so a plan
/// can be diffed and re-reviewed.
pub fn scan(root: &std::path::Path) -> Vec<Entry> {
    let mut entries: Vec<Entry> = WalkDir::new(root)
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
        .collect();

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}
