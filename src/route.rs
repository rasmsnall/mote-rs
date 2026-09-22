use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::archive::{self, Inspection};
use crate::dedupe::Group;
use crate::ident;
use crate::plan::{Action, PlannedMember};
use crate::rules::Rules;
use crate::walk::Entry;

/// Everything the routing pass decided.
pub struct Routed {
    pub actions: Vec<Action>,
    pub unroutable: Vec<PathBuf>,
}

/// Decide what happens to every scanned file.
///
/// Duplicates are settled first so that only one copy of a given content is
/// ever routed; the rest are quarantined. Whatever survives is identified by
/// content and matched against the rules. Archives, when `extract` is on, are
/// opened and their members routed individually, as though each had been
/// found loose on disk.
///
/// Nothing here touches the filesystem beyond reading.
pub fn build(
    root: &Path,
    entries: &[Entry],
    groups: &[Group],
    rules: &Rules,
    extract: bool,
) -> Routed {
    let mut actions = Vec::new();
    let mut unroutable = Vec::new();
    let mut claimed: HashSet<PathBuf> = HashSet::new();

    // Every copy but one in each duplicate group. The survivor is the
    // lexicographically first path, which keeps a re-scan of an unchanged tree
    // producing a byte-identical plan.
    let mut superseded: HashMap<&Path, &str> = HashMap::new();
    for group in groups {
        for path in group.paths.iter().skip(1) {
            superseded.insert(path.as_path(), group.hash.as_str());
        }
    }

    for entry in entries {
        let rel = entry.path.strip_prefix(root).unwrap_or(&entry.path);

        if let Some(hash) = superseded.get(entry.path.as_path()) {
            // Quarantine mirrors the source layout, so two duplicates with the
            // same basename cannot collide once set aside.
            actions.push(Action::Quarantine {
                from: entry.path.clone(),
                to: rules.settings.quarantine.join(rel),
                reason: format!("duplicate of content {}", &hash[..16.min(hash.len())]),
            });
            continue;
        }

        let kind = ident::identify(&entry.path);

        if extract {
            if let Some(format) = kind.archive() {
                match archive::inspect(&entry.path, format, &rules.settings) {
                    Ok(Inspection::Contents(members)) => {
                        let mut planned = Vec::with_capacity(members.len());
                        for member in &members {
                            match rules.route(&member.name, &member.kind) {
                                Some(dir) => planned.push(PlannedMember {
                                    to: claim(&mut claimed, &dir, &member.name),
                                    name: member.name.clone(),
                                    size: member.size,
                                }),
                                None => unroutable.push(entry.path.join(&member.name)),
                            }
                        }
                        if !planned.is_empty() {
                            actions.push(Action::Extract {
                                from: entry.path.clone(),
                                format,
                                members: planned,
                            });
                        }
                    }
                    Ok(Inspection::Rejected(reason)) => {
                        actions.push(Action::Quarantine {
                            from: entry.path.clone(),
                            to: rules.settings.quarantine.join(rel),
                            reason,
                        });
                    }
                    Err(e) => {
                        actions.push(Action::Quarantine {
                            from: entry.path.clone(),
                            to: rules.settings.quarantine.join(rel),
                            reason: format!("could not be read as an archive: {e}"),
                        });
                    }
                }
                continue;
            }
        }

        match rules.route(rel, &kind) {
            Some(dir) => {
                let to = claim(&mut claimed, &dir, rel);
                actions.push(Action::Move {
                    from: entry.path.clone(),
                    to,
                });
            }
            None => unroutable.push(entry.path.clone()),
        }
    }

    Routed {
        actions,
        unroutable,
    }
}

/// Pick a free destination inside `dir` for a file named after `source`.
///
/// Routing flattens: a file's destination is its rule's directory plus its own
/// basename, wherever it came from. Two different files can therefore want the
/// same name, so the second gets a `-2` before its extension. `apply` refuses
/// to clobber as well, but resolving it here means the collision is visible in
/// the plan rather than surfacing as a skipped file at commit time.
fn claim(claimed: &mut HashSet<PathBuf>, dir: &Path, source: &Path) -> PathBuf {
    let name = source
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("unnamed"));

    let stem = name
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed".into());
    let ext = name.extension().map(|e| e.to_string_lossy().into_owned());

    for n in 1u32.. {
        let candidate = if n == 1 {
            name.to_string_lossy().into_owned()
        } else {
            match &ext {
                Some(ext) => format!("{stem}-{n}.{ext}"),
                None => format!("{stem}-{n}"),
            }
        };
        let path = dir.join(candidate);
        if claimed.insert(path.clone()) {
            return path;
        }
    }
    unreachable!("u32 exhausted picking a filename")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colliding_names_get_suffixed_not_dropped() {
        let mut claimed = HashSet::new();
        let dir = Path::new("/out/images");

        let a = claim(&mut claimed, dir, Path::new("one/photo.jpg"));
        let b = claim(&mut claimed, dir, Path::new("two/photo.jpg"));
        let c = claim(&mut claimed, dir, Path::new("three/photo.jpg"));

        assert_eq!(a, PathBuf::from("/out/images/photo.jpg"));
        assert_eq!(b, PathBuf::from("/out/images/photo-2.jpg"));
        assert_eq!(c, PathBuf::from("/out/images/photo-3.jpg"));
    }

    #[test]
    fn extensionless_collisions_still_resolve() {
        let mut claimed = HashSet::new();
        let dir = Path::new("/out/misc");

        assert_eq!(
            claim(&mut claimed, dir, Path::new("LICENSE")),
            PathBuf::from("/out/misc/LICENSE")
        );
        assert_eq!(
            claim(&mut claimed, dir, Path::new("sub/LICENSE")),
            PathBuf::from("/out/misc/LICENSE-2")
        );
    }
}
