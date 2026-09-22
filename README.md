# mote

Filesystem triage agent. Walks a tree, finds byte-identical duplicates, and
(once routing lands) moves files to where they belong.

## Design

**Scan and apply are separate commands.** `scan` never writes to the tree it is
reading; it emits a JSON plan. `apply` executes that plan, and only with an
explicit `--commit`. So dry-run is the default, the plan is reviewable before
anything moves, and a interrupted run can be resumed.

**Duplicate detection is tiered.** Files are bucketed by size, then by a hash of
their first and last 64 KB, and only survivors are hashed in full. Nearly all
candidates are eliminated before any bytes are read.

Symlinks are never followed — a link farm would manufacture phantom duplicates,
and following one could walk out of the mounted volume.

## Usage

```
mote scan /data --out plan.json
mote apply plan.json              # prints what it would do
mote apply plan.json --commit     # actually moves files
```

## Docker

Mount read-only for the scan, so the phase that cannot damage anything is
enforced by the container rather than by trust in the code:

```
docker run --rm -v /data:/data:ro -v "$PWD:/out" \
  --user "$(id -u):$(id -g)" mote scan /data --out /out/plan.json
```

Then read-write only for the apply:

```
docker run --rm -v /data:/data -v "$PWD:/out" \
  --user "$(id -u):$(id -g)" mote apply /out/plan.json --commit
```

The `--user` flag matters: without it every moved file ends up owned by root.

## Not yet decided

Routing is stubbed (`actions` is always empty). The open question is whether
archives are **items to file** — move `foo.zip` based on what it contains,
reading only the central directory — or **things to extract**, which needs a
disk-space budget and zip-bomb guards. These are different tools; the rest of
the pipeline is the same either way.
