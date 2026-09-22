use std::path::Path;

use anyhow::{Context, Result};

use crate::plan::{Action, Plan};

/// Execute a plan.
///
/// Renames are attempted first and fall back to copy-then-remove, because a
/// scan will routinely span mount points where `rename(2)` returns `EXDEV`.
/// An existing destination is never clobbered.
pub fn run(plan: &Plan) -> Result<usize> {
    let mut done = 0;

    for action in &plan.actions {
        let (from, to) = match action {
            Action::Move { from, to } | Action::Quarantine { from, to } => (from, to),
        };

        if to.exists() {
            eprintln!("skip (destination exists): {}", to.display());
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        relocate(from, to).with_context(|| {
            format!("moving {} -> {}", from.display(), to.display())
        })?;
        done += 1;
    }

    Ok(done)
}

fn relocate(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
    }
}
