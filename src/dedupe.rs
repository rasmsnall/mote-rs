use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::walk::Entry;

/// Bytes read from each end of a file for the cheap second-tier hash.
const EDGE: u64 = 64 * 1024;

/// A set of files with identical content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub hash: String,
    pub size: u64,
    pub paths: Vec<PathBuf>,
}

/// Find groups of byte-identical files.
///
/// Three tiers, cheapest first: files are bucketed by size, then by a hash of
/// their first and last 64 KB, and only the survivors are hashed in full. Most
/// candidates die at tier one for free.
pub fn find(entries: &[Entry]) -> Vec<Group> {
    let by_size = bucket(entries.iter().cloned(), |e| e.size);

    let candidates: Vec<Entry> = by_size
        .into_iter()
        .filter(|(size, group)| *size > 0 && group.len() > 1)
        .flat_map(|(_, group)| group)
        .collect();

    let edged: Vec<(u64, [u8; 32], Entry)> = candidates
        .par_iter()
        .filter_map(|e| edge_hash(e).ok().map(|h| (e.size, h, e.clone())))
        .collect();

    let by_edge = bucket(edged.into_iter(), |(size, hash, _)| (*size, *hash));

    let full: Vec<(String, Entry)> = by_edge
        .into_iter()
        .filter(|(_, group)| group.len() > 1)
        .flat_map(|(_, group)| group)
        .collect::<Vec<_>>()
        .par_iter()
        .filter_map(|(_, _, e)| full_hash(e).ok().map(|h| (h, e.clone())))
        .collect();

    let mut groups: Vec<Group> = bucket(full.into_iter(), |(hash, _)| hash.clone())
        .into_iter()
        .filter(|(_, group)| group.len() > 1)
        .map(|(hash, group)| Group {
            hash,
            size: group[0].1.size,
            paths: group.into_iter().map(|(_, e)| e.path).collect(),
        })
        .collect();

    // Largest wasted space first: that is the order a human wants to review in.
    groups.sort_by_key(|g| std::cmp::Reverse(g.size * (g.paths.len() as u64 - 1)));
    groups
}

fn bucket<T, K, F>(items: impl Iterator<Item = T>, key: F) -> HashMap<K, Vec<T>>
where
    K: std::hash::Hash + Eq,
    F: Fn(&T) -> K,
{
    let mut map: HashMap<K, Vec<T>> = HashMap::new();
    for item in items {
        map.entry(key(&item)).or_default().push(item);
    }
    map
}

fn edge_hash(entry: &Entry) -> std::io::Result<[u8; 32]> {
    let mut file = File::open(&entry.path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; EDGE.min(entry.size) as usize];

    file.read_exact(&mut buf)?;
    hasher.update(&buf);

    if entry.size > EDGE * 2 {
        file.seek(SeekFrom::End(-(EDGE as i64)))?;
        file.read_exact(&mut buf)?;
        hasher.update(&buf);
    }

    Ok(*hasher.finalize().as_bytes())
}

fn full_hash(entry: &Entry) -> std::io::Result<String> {
    let mut file = File::open(&entry.path)?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}
