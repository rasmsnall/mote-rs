use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::ident::Kind;

/// A parsed `rules.toml`.
///
/// Rules are ordered and the first match wins. That makes the file readable
/// top-to-bottom as a decision list rather than a set of competing patterns
/// whose precedence a user has to reason about.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rules {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
    /// Compiled globs, one per rule, built at load time.
    #[serde(skip)]
    globs: Vec<Option<GlobSet>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Settings {
    /// Root under which relative rule destinations resolve.
    pub dest: PathBuf,
    /// Where duplicates and unsafe archive members are set aside.
    pub quarantine: PathBuf,
    /// Reject an archive whose members expand to more than this many bytes.
    #[serde(default = "default_budget")]
    pub extract_budget: u64,
    /// Reject an archive whose expansion exceeds this multiple of its own size.
    #[serde(default = "default_ratio")]
    pub max_ratio: u64,
    /// Reject an archive holding more than this many members.
    #[serde(default = "default_max_members")]
    pub max_members: usize,
}

fn default_budget() -> u64 {
    8 * 1024 * 1024 * 1024
}
fn default_ratio() -> u64 {
    100
}
fn default_max_members() -> usize {
    10_000
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dest: PathBuf::from("sorted"),
            quarantine: PathBuf::from("sorted/.mote-quarantine"),
            extract_budget: default_budget(),
            max_ratio: default_ratio(),
            max_members: default_max_members(),
        }
    }
}

/// One line of the decision list.
///
/// A rule with no matcher at all matches everything, which is how a catch-all
/// is written. Within a rule the matchers are OR-ed: any one of them hitting
/// selects the rule.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Rule {
    /// MIME types. A value ending in `/` matches by prefix, e.g. `image/`.
    #[serde(default)]
    pub r#type: Vec<String>,
    /// Bare extensions, without the dot, compared case-insensitively.
    #[serde(default)]
    pub ext: Vec<String>,
    /// Globs matched against the file name and the path relative to the root.
    #[serde(default)]
    pub glob: Vec<String>,
    /// Destination, relative to `settings.dest` unless absolute.
    pub to: PathBuf,
}

impl Rules {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut rules: Rules =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        rules.compile()?;
        Ok(rules)
    }

    /// Compile every rule's globs once, so matching a file is not a parse.
    fn compile(&mut self) -> Result<()> {
        self.globs = Vec::with_capacity(self.rules.len());
        for (i, rule) in self.rules.iter().enumerate() {
            if rule.glob.is_empty() {
                self.globs.push(None);
                continue;
            }
            let mut builder = GlobSetBuilder::new();
            for pattern in &rule.glob {
                builder.add(
                    Glob::new(pattern)
                        .with_context(|| format!("rule {i}: bad glob {pattern:?}"))?,
                );
            }
            self.globs.push(Some(builder.build()?));
        }
        if self.rules.is_empty() {
            bail!("no [[rule]] entries: every file would be unroutable");
        }
        Ok(())
    }

    /// The destination directory for a file, or `None` if no rule matches.
    ///
    /// `rel` is the path relative to the scan root; `kind` is what the file
    /// actually is, which may disagree with its extension.
    pub fn route(&self, rel: &Path, kind: &Kind) -> Option<PathBuf> {
        let name = rel.file_name()?.to_string_lossy().to_lowercase();
        let ext = rel
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        for (i, rule) in self.rules.iter().enumerate() {
            if rule.matches(kind, &ext, self.globs[i].as_ref(), rel, &name) {
                return Some(self.resolve(&rule.to));
            }
        }
        None
    }

    /// Make a rule destination absolute against `settings.dest`.
    pub fn resolve(&self, to: &Path) -> PathBuf {
        if to.is_absolute() {
            to.to_path_buf()
        } else {
            self.settings.dest.join(to)
        }
    }
}

impl Rule {
    fn matches(
        &self,
        kind: &Kind,
        ext: &str,
        globs: Option<&GlobSet>,
        rel: &Path,
        name: &str,
    ) -> bool {
        // A rule with no matchers is a catch-all.
        if self.r#type.is_empty() && self.ext.is_empty() && self.glob.is_empty() {
            return true;
        }

        let mime = kind.mime();
        if self.r#type.iter().any(|t| {
            if let Some(prefix) = t.strip_suffix('/') {
                mime.split('/').next() == Some(prefix)
            } else {
                mime.eq_ignore_ascii_case(t)
            }
        }) {
            return true;
        }

        if !ext.is_empty() && self.ext.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            return true;
        }

        if let Some(set) = globs {
            if set.is_match(rel) || set.is_match(name) {
                return true;
            }
        }

        false
    }
}

/// The rules file written by `mote init`, and the documentation of the format.
pub const TEMPLATE: &str = r#"# mote routing rules.
#
# Rules are tried top to bottom and the first match wins, so put the specific
# ones first and a catch-all last. Within one rule, `type`, `ext` and `glob`
# are OR-ed together.
#
# `type` is the MIME type mote detected from the file's CONTENT, not from its
# name -- that is the point of the tool. A value ending in `/` matches a whole
# family, so `image/` catches jpeg, png, webp and the rest.

[settings]
# Relative `to =` destinations resolve under this directory.
dest = "/out"
# Duplicates and rejected archive members land here. Nothing is ever deleted.
quarantine = "/out/.mote-quarantine"
# Refuse to extract an archive expanding past these limits.
extract_budget = 8589934592   # 8 GiB of output across one archive
max_ratio = 100               # 100x its own compressed size
max_members = 10000

[[rule]]
type = ["image/"]
to = "images"

[[rule]]
type = ["video/"]
to = "video"

[[rule]]
type = ["audio/"]
to = "audio"

[[rule]]
type = ["application/pdf"]
ext = ["doc", "docx", "odt", "rtf", "epub"]
to = "documents"

[[rule]]
glob = ["*.rs", "*.py", "*.go", "*.ts", "*.js", "*.c", "*.h", "*.toml", "*.json", "*.yaml", "*.yml"]
to = "code"

# Catch-all: no matchers, so everything still unclaimed lands here. Without a
# rule like this, unmatched files are reported as unroutable and left alone.
[[rule]]
to = "unsorted"
"#;
