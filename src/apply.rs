use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::archive;
use crate::plan::{Action, Plan};

/// Execute a plan.
///
/// Every step is skip-if-done rather than fail-if-done, so a run interrupted
/// halfway can simply be run again: moves whose destination already exists are
/// left alone, and archive members already written are not rewritten. An
/// existing destination is never clobbered.
pub fn run(plan: &Plan) -> Result<usize> {
    let mut done = 0;

    for action in &plan.actions {
        match action {
            Action::Move { from, to } | Action::Quarantine { from, to, .. } => {
                if !from.exists() {
                    // Already moved by an earlier run.
                    continue;
                }
                if to.exists() {
                    eprintln!("skip (destination exists): {}", to.display());
                    continue;
                }
                parent_of(to)?;
                relocate(from, to)
                    .with_context(|| format!("moving {} -> {}", from.display(), to.display()))?;
                done += 1;
            }

            Action::Extract {
                from,
                format,
                members,
            } => {
                // The plan is the allow-list. `extract` writes a member only
                // if it appears here, so an archive that changed between scan
                // and apply cannot smuggle in a path nobody reviewed.
                let allowed: HashMap<&Path, &PathBuf> =
                    members.iter().map(|m| (m.name.as_path(), &m.to)).collect();

                let written = archive::extract(from, *format, &|name: &Path| {
                    allowed.get(name).map(|p| (*p).clone())
                })
                .with_context(|| format!("extracting {}", from.display()))?;

                eprintln!(
                    "extracted {written}/{} members from {}",
                    members.len(),
                    from.display()
                );
                done += written;
            }
        }
    }

    Ok(done)
}

fn parent_of(to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    Ok(())
}

/// Renames are attempted first and fall back to copy-then-remove, because a
/// scan will routinely span mount points where `rename(2)` returns `EXDEV`.
/// The source is removed only once the copy has fully succeeded.
fn relocate(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
    }
}
