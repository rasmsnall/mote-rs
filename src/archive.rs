use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::ident::{self, Format, Kind};
use crate::rules::Settings;

/// Bytes of each member sampled to identify it during inspection.
const PEEK: usize = 8192;

/// One file inside an archive, as seen without extracting it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    /// The member's path inside the archive, sanitised and relative.
    pub name: PathBuf,
    /// Uncompressed size the archive's own header claims.
    pub size: u64,
    /// What the member's leading bytes say it is.
    pub kind: Kind,
}

/// The outcome of looking inside an archive.
#[derive(Debug, Clone)]
pub enum Inspection {
    /// Safe to expand; these are its files.
    Contents(Vec<Member>),
    /// Refused. The archive gets quarantined whole and a human decides.
    Rejected(String),
}

/// List an archive's contents without writing anything to disk.
///
/// Every limit in `settings` is checked here, at plan time, so that a refusal
/// shows up in the JSON plan a human reviews rather than halfway through a
/// commit. Member bodies are decompressed only far enough to identify them
/// (`PEEK` bytes), never to their full size.
pub fn inspect(path: &Path, format: Format, settings: &Settings) -> Result<Inspection> {
    let archive_size = std::fs::metadata(path)?.len();
    let members = match format {
        Format::Zip => list_zip(path, settings)?,
        Format::Tar => list_tar(path, settings, false)?,
        Format::TarGz => list_tar(path, settings, true)?,
    };

    let members = match members {
        Inspection::Contents(m) => m,
        rejected => return Ok(rejected),
    };

    let total: u64 = members.iter().map(|m| m.size).sum();
    if total > settings.extract_budget {
        return Ok(Inspection::Rejected(format!(
            "expands to {total} bytes, over the {} byte budget",
            settings.extract_budget
        )));
    }
    // A gzip bomb is small on disk and enormous once expanded. Compare the two
    // rather than trusting either number alone.
    if archive_size > 0 && total / archive_size.max(1) > settings.max_ratio {
        return Ok(Inspection::Rejected(format!(
            "expands {}x its own size, over the {}x limit",
            total / archive_size.max(1),
            settings.max_ratio
        )));
    }

    Ok(Inspection::Contents(members))
}

fn list_zip(path: &Path, settings: &Settings) -> Result<Inspection> {
    let file = File::open(path)?;
    let mut zip = match zip::ZipArchive::new(BufReader::new(file)) {
        Ok(z) => z,
        Err(e) => return Ok(Inspection::Rejected(format!("unreadable zip: {e}"))),
    };

    if zip.len() > settings.max_members {
        return Ok(Inspection::Rejected(format!(
            "holds {} members, over the {} limit",
            zip.len(),
            settings.max_members
        )));
    }

    let mut members = Vec::new();
    for i in 0..zip.len() {
        let mut entry = match zip.by_index(i) {
            Ok(e) => e,
            Err(e) => return Ok(Inspection::Rejected(format!("member {i}: {e}"))),
        };
        if entry.is_dir() {
            continue;
        }

        let raw = entry.name().to_string();
        let Some(name) = safe_name(&raw) else {
            return Ok(Inspection::Rejected(format!(
                "member {raw:?} escapes the extraction directory"
            )));
        };

        let size = entry.size();
        let mut head = vec![0u8; PEEK.min(size as usize)];
        let read = read_head(&mut entry, &mut head);
        head.truncate(read);

        members.push(Member {
            kind: ident::identify_bytes(&head, &name),
            name,
            size,
        });
    }

    Ok(Inspection::Contents(members))
}

fn list_tar(path: &Path, settings: &Settings, gzip: bool) -> Result<Inspection> {
    let file = BufReader::new(File::open(path)?);
    let reader: Box<dyn Read> = if gzip {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut tar = tar::Archive::new(reader);

    let mut members = Vec::new();
    let mut expanded: u64 = 0;

    let entries = match tar.entries() {
        Ok(e) => e,
        Err(e) => return Ok(Inspection::Rejected(format!("unreadable tar: {e}"))),
    };

    for entry in entries {
        let mut entry = match entry {
            Ok(e) => e,
            Err(e) => return Ok(Inspection::Rejected(format!("bad tar entry: {e}"))),
        };

        // Only regular files. A tar can carry symlinks, hardlinks, devices and
        // fifos; every one of them is a way to write outside the destination
        // or to hand the kernel something it should not get.
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }

        let raw = entry.path()?.to_string_lossy().into_owned();
        let Some(name) = safe_name(&raw) else {
            return Ok(Inspection::Rejected(format!(
                "member {raw:?} escapes the extraction directory"
            )));
        };

        let size = entry.size();

        // A tar is read start to finish, so the budget is enforced as we go
        // instead of after a full listing: a bomb must not be able to make us
        // stream terabytes just to count them.
        expanded = expanded.saturating_add(size);
        if expanded > settings.extract_budget {
            return Ok(Inspection::Rejected(format!(
                "expands past the {} byte budget",
                settings.extract_budget
            )));
        }
        if members.len() >= settings.max_members {
            return Ok(Inspection::Rejected(format!(
                "holds more than {} members",
                settings.max_members
            )));
        }

        let mut head = vec![0u8; PEEK.min(size as usize)];
        let read = read_head(&mut entry, &mut head);
        head.truncate(read);

        members.push(Member {
            kind: ident::identify_bytes(&head, &name),
            name,
            size,
        });
    }

    Ok(Inspection::Contents(members))
}

/// Fill `buf` as far as the member allows. A short or failing read just means
/// a less confident identification, never a failed scan.
fn read_head(reader: &mut impl Read, buf: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => break,
        }
    }
    filled
}

/// Reduce an archive member's declared path to something that cannot escape.
///
/// Returns `None` for anything that tries: absolute paths, `..` traversal,
/// Windows drive letters, and embedded NULs. Backslashes are treated as
/// separators too, because an archive written on Windows uses them and a
/// crafted one uses them to sneak `..\..` past a `/`-only check.
fn safe_name(raw: &str) -> Option<PathBuf> {
    if raw.is_empty() || raw.contains('\0') {
        return None;
    }
    if raw.starts_with('/') || raw.starts_with('\\') {
        return None;
    }
    // `C:` or any other drive-letter prefix.
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return None;
    }

    let mut out = PathBuf::new();
    for part in raw.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return None,
            other => out.push(other),
        }
    }

    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Stream one planned member out of `path` to `to`.
///
/// `plan` maps a member's in-archive name to where it goes. A member absent
/// from the map is skipped: `apply` writes only files the reviewed plan named,
/// so an archive swapped between scan and apply cannot introduce new ones.
/// Each member is capped at its declared size, so a header that lies about how
/// small it is cannot run the disk out.
pub fn extract(
    path: &Path,
    format: Format,
    plan: &dyn Fn(&Path) -> Option<PathBuf>,
) -> Result<usize> {
    match format {
        Format::Zip => extract_zip(path, plan),
        Format::Tar => extract_tar(path, plan, false),
        Format::TarGz => extract_tar(path, plan, true),
    }
}

fn extract_zip(path: &Path, plan: &dyn Fn(&Path) -> Option<PathBuf>) -> Result<usize> {
    let mut zip = zip::ZipArchive::new(BufReader::new(File::open(path)?))
        .with_context(|| format!("opening {}", path.display()))?;
    let mut written = 0;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let Some(name) = safe_name(entry.name()) else {
            continue;
        };
        let Some(to) = plan(&name) else { continue };

        let size = entry.size();
        if write_member(&mut entry, &to, size)? {
            written += 1;
        }
    }
    Ok(written)
}

fn extract_tar(path: &Path, plan: &dyn Fn(&Path) -> Option<PathBuf>, gzip: bool) -> Result<usize> {
    let file = BufReader::new(File::open(path)?);
    let reader: Box<dyn Read> = if gzip {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut tar = tar::Archive::new(reader);
    let mut written = 0;

    for entry in tar.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let raw = entry.path()?.to_string_lossy().into_owned();
        let Some(name) = safe_name(&raw) else {
            continue;
        };
        let Some(to) = plan(&name) else { continue };

        let size = entry.size();
        if write_member(&mut entry, &to, size)? {
            written += 1;
        }
    }
    Ok(written)
}

/// Write one member, refusing to overwrite and refusing to exceed `size`.
fn write_member(reader: &mut impl Read, to: &Path, size: u64) -> Result<bool> {
    if to.exists() {
        eprintln!("skip (destination exists): {}", to.display());
        return Ok(false);
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    // Write to a temporary sibling first: a crash mid-extraction then leaves
    // no half-written file that the next run would mistake for finished work.
    let temp = to.with_extension("mote-partial");
    let mut out = File::create(&temp).with_context(|| format!("creating {}", temp.display()))?;
    let copied = std::io::copy(&mut reader.take(size), &mut out);

    match copied {
        Ok(n) if n == size => {
            drop(out);
            std::fs::rename(&temp, to).with_context(|| format!("finishing {}", to.display()))?;
            Ok(true)
        }
        Ok(n) => {
            drop(out);
            let _ = std::fs::remove_file(&temp);
            bail!(
                "{}: archive declared {size} bytes but held {n}",
                to.display()
            )
        }
        Err(e) => {
            drop(out);
            let _ = std::fs::remove_file(&temp);
            Err(e).with_context(|| format!("writing {}", to.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_is_refused() {
        assert_eq!(safe_name("../../etc/passwd"), None);
        assert_eq!(safe_name("a/../../b"), None);
        assert_eq!(safe_name("/etc/passwd"), None);
        assert_eq!(safe_name("\\windows\\system32"), None);
        assert_eq!(safe_name("C:/Windows"), None);
        assert_eq!(safe_name("a/..\\b"), None);
        assert_eq!(safe_name(""), None);
        assert_eq!(safe_name("bad\0name"), None);
    }

    #[test]
    fn ordinary_names_survive_intact() {
        assert_eq!(safe_name("a/b/c.txt"), Some(PathBuf::from("a/b/c.txt")));
        assert_eq!(safe_name("./a/./b.txt"), Some(PathBuf::from("a/b.txt")));
        assert_eq!(safe_name("photo.jpg"), Some(PathBuf::from("photo.jpg")));
    }

    #[test]
    fn a_dot_only_name_is_not_a_file() {
        assert_eq!(safe_name("."), None);
        assert_eq!(safe_name("./"), None);
    }
}
