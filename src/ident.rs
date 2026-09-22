use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Bytes read from the head of a file to identify it. Every signature `infer`
/// knows about fits comfortably inside this.
const PEEK: usize = 8192;

/// What a file actually is.
///
/// Detection reads the file's leading bytes and only consults the name when
/// the content is unrecognised. A `.jpg` holding a zip is a zip here, which is
/// the whole reason the tool looks at bytes at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Kind {
    /// Identified from magic bytes.
    Magic { mime: String },
    /// Magic bytes were unrecognised; this is the extension's claim.
    Extension { mime: String },
    /// No signature and no useful extension.
    Unknown,
}

impl Kind {
    pub fn mime(&self) -> &str {
        match self {
            Kind::Magic { mime } | Kind::Extension { mime } => mime,
            Kind::Unknown => "application/octet-stream",
        }
    }

    /// Whether this is a container `mote` knows how to expand.
    pub fn archive(&self) -> Option<Format> {
        // Only trust magic bytes here. Extracting something because it was
        // *named* `.zip` means handing a parser bytes it did not expect.
        let Kind::Magic { mime } = self else {
            return None;
        };
        match mime.as_str() {
            "application/zip" => Some(Format::Zip),
            "application/x-tar" => Some(Format::Tar),
            "application/gzip" => Some(Format::TarGz),
            _ => None,
        }
    }
}

/// An archive container mote can expand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Zip,
    Tar,
    TarGz,
}

/// Identify the file at `path`.
///
/// Reads at most `PEEK` bytes. An unreadable file is `Unknown` rather than an
/// error: one bad file should not abort a scan of a million.
pub fn identify(path: &Path) -> Kind {
    let mut buf = [0u8; PEEK];
    let read = File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);

    if read > 0 {
        if let Some(t) = infer::get(&buf[..read]) {
            return Kind::Magic {
                mime: t.mime_type().to_string(),
            };
        }
    }

    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => match from_extension(&ext.to_lowercase()) {
            Some(mime) => Kind::Extension {
                mime: mime.to_string(),
            },
            None => Kind::Unknown,
        },
        None => Kind::Unknown,
    }
}

/// Identify from bytes already in hand, for archive members that are never
/// written to disk before their destination is known.
pub fn identify_bytes(head: &[u8], name: &Path) -> Kind {
    if let Some(t) = infer::get(head) {
        return Kind::Magic {
            mime: t.mime_type().to_string(),
        };
    }
    match name.extension().and_then(|e| e.to_str()) {
        Some(ext) => match from_extension(&ext.to_lowercase()) {
            Some(mime) => Kind::Extension {
                mime: mime.to_string(),
            },
            None => Kind::Unknown,
        },
        None => Kind::Unknown,
    }
}

/// Extensions worth knowing that carry no magic bytes.
///
/// `infer` covers binary formats well but recognises almost no text, and text
/// is most of what a source tree holds.
fn from_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "txt" | "log" | "text" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        "c" | "h" => "text/x-c",
        "cpp" | "cc" | "hpp" => "text/x-c++",
        "go" => "text/x-go",
        "js" | "mjs" | "cjs" => "text/javascript",
        "ts" | "tsx" => "text/x-typescript",
        "sh" | "bash" | "zsh" => "application/x-sh",
        "sql" => "application/sql",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn magic_beats_a_lying_extension() {
        // A PNG signature in a file named `.txt` is still a PNG.
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
        let kind = identify_bytes(&png, &PathBuf::from("notes.txt"));
        assert_eq!(
            kind,
            Kind::Magic {
                mime: "image/png".into()
            }
        );
    }

    #[test]
    fn extension_is_the_fallback_not_the_first_word() {
        let kind = identify_bytes(b"# hello\n", &PathBuf::from("README.md"));
        assert_eq!(
            kind,
            Kind::Extension {
                mime: "text/markdown".into()
            }
        );
    }

    #[test]
    fn archives_are_only_trusted_from_magic() {
        let named = Kind::Extension {
            mime: "application/zip".into(),
        };
        assert_eq!(named.archive(), None);

        let sniffed = Kind::Magic {
            mime: "application/zip".into(),
        };
        assert_eq!(sniffed.archive(), Some(Format::Zip));
    }
}
