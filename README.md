# mote

Filesystem triage agent. Walks a tree, identifies files by their contents,
routes them to where they belong, expands archives, and sets duplicates aside.

## Design

**Scan and apply are separate commands.** `scan` never writes to the tree it is
reading; it emits a JSON plan. `apply` executes that plan, and only with an
explicit `--commit`. So dry-run is the default, the plan is reviewable before
anything moves, and an interrupted run can be resumed.

**Files are identified by content, not by name.** Detection reads the leading
bytes and consults the extension only when the signature is unrecognised. A PNG
named `notes.txt` is routed as a PNG — catching exactly that case is the reason
the tool exists. Extensions still cover text formats, which carry no magic
bytes.

**Duplicate detection is tiered.** Files are bucketed by size, then by a hash of
their first and last 64 KB, and only survivors are hashed in full. Nearly all
candidates are eliminated before their bytes are read. Duplicates are
quarantined, never deleted: one copy is routed normally and the rest move to a
quarantine directory that mirrors the source layout. Emptying it is your call.

**Plans are deterministic.** The parallel walk is sorted before anything
downstream consumes it, so two scans of an unchanged tree produce byte-identical
plans. That is what makes a plan diffable and a re-review cheap.

**Nothing is clobbered and nothing is deleted.** `apply` skips any destination
that already exists. When two different files want the same destination name,
the collision is resolved at plan time with a `-2` suffix, so it is visible in
the plan rather than surfacing as a silently skipped file at commit time.

Symlinks are never followed — a link farm would manufacture phantom duplicates,
and following one could walk out of the mounted volume.

## Archives

With `--extract`, archives are opened and each member is routed individually, as
though it had been found loose on disk. Zip, tar and tar.gz are supported.

Member destinations are resolved **at scan time**, so the plan names every path
that will be written before anything is. `apply` treats that list as an
allow-list: a member absent from it is skipped, so an archive swapped between
scan and apply cannot introduce a path nobody reviewed.

An archive that trips any of these is quarantined whole, with the reason
recorded in the plan, and a human decides:

| Guard | Default | What it stops |
|---|---|---|
| `extract_budget` | 8 GiB | An archive expanding past a fixed ceiling. |
| `max_ratio` | 100× | A bomb: small on disk, enormous once expanded. |
| `max_members` | 10,000 | An archive with a pathological number of entries. |
| path sanitisation | always | `../` traversal, absolute paths, drive letters, backslash separators and embedded NULs. |

Tars carry symlinks, hardlinks, devices and fifos; only regular files are
extracted. Each member is capped at the size its header declared, so a lying
header cannot run the disk out, and members are written to a temporary sibling
and renamed, so an interrupted extraction leaves no half-written file that the
next run mistakes for finished work.

Archives are **left in place** after a successful extraction. Deleting the only
copy of a container on the strength of an extraction nobody has checked yet is
not recoverable. Re-scanning is safe: the second apply skips every member that
is already there.

Nested archives are not expanded recursively — an archive inside an archive is
written out as a file, and a second scan will expand it.

## Rules

`mote init` writes a starter `rules.toml`. Rules are tried top to bottom and the
first match wins, so the file reads as a decision list:

```toml
[settings]
dest = "/out"
quarantine = "/out/.mote-quarantine"

[[rule]]
type = ["image/"]        # trailing slash matches a whole MIME family
to = "images"

[[rule]]
type = ["application/pdf"]
ext  = ["doc", "docx", "epub"]
to = "documents"

[[rule]]
glob = ["*.rs", "*.py", "*.toml"]
to = "code"

[[rule]]                 # no matchers: a catch-all
to = "unsorted"
```

Within one rule, `type`, `ext` and `glob` are OR-ed. `type` is the MIME type
detected from content. Files no rule claims are listed in the plan's
`unroutable` array and left where they are.

## Usage

```
mote init                                   # write a starter rules.toml
mote scan /data --out plan.json             # read-only; writes a plan
mote scan /data --out plan.json --extract   # also expand archives
mote apply plan.json                        # prints what it would do
mote apply plan.json --commit               # actually moves files
```

## Docker

Mount read-only for the scan, so the phase that cannot damage anything is
enforced by the container rather than by trust in the code:

```
docker run --rm -v /data:/data:ro -v "$PWD:/out" \
  --user "$(id -u):$(id -g)" mote scan /data --rules /out/rules.toml --out /out/plan.json
```

Then read-write only for the apply:

```
docker run --rm -v /data:/data -v "$PWD:/out" \
  --user "$(id -u):$(id -g)" mote apply /out/plan.json --commit
```

The `--user` flag matters: without it every moved file ends up owned by root.

Note that `--extract` makes the scan decompress member heads in memory to
identify them. It still writes nothing, so the read-only mount remains correct.
