# 0011. Shared content-addressable model store

- Status: accepted
- Date: 2026-05-18

## Context

[ADR 0010](0010-narrow-https-exception-for-model-bootstrap.md)
authorised the daemon to fetch a single pinned GGUF on first
boot. It said nothing about *where* the bytes land on disk. The
implementation that shipped used `models_dir` from the config
file, with a flat `<dir>/<filename>` layout — fine in isolation,
but it bakes inferd into the same per-tool silo every other
local-AI runtime sits in:

- Ollama writes to `~/.ollama/models/blobs/sha256-…`
- LM Studio writes to `~/.cache/lm-studio/models/.../`
- HuggingFace Transformers writes to `~/.cache/huggingface/hub/`
- llama.cpp users pick whatever directory they remember

A user running four such tools holds the same multi-GB GGUF
four times on disk. The blobs are byte-identical; the
duplication is a coordination failure, not a technical
constraint. At 5 GB per Q4_K_M and 40 GB per 70B-class model,
this isn't a rounding error — it's the dominant disk-usage line
item for any serious local-AI user.

A separate spec proposal (sibling document, *Shared Local
Model Store — Cross-Tool Convention Proposal*) lays out the
shape such a store should take. inferd is in a useful position
to adopt it on day one rather than retrofit later: the daemon
has not shipped, no users are pinned to the flat layout, and
the existing fetch module is small.

## Decision

inferd writes downloaded models into a shared, content-
addressable store at a platform-conventional location. The
layout matches the cross-tool proposal verbatim so that
manifests written by inferd are readable by — and blobs
fetched by inferd are reusable by — any other tool that
adopts the same convention.

### 1. Where the store lives

Resolution order, first hit wins:

1. `models_home` field in `~/.inferd/config.json` if set.
2. `MODELS_HOME` environment variable if set.
3. Platform default:

   | Platform | Default |
   |---|---|
   | Linux / *BSD | `${XDG_DATA_HOME:-$HOME/.local/share}/models/` |
   | macOS | `~/Library/Application Support/models/` |
   | Windows | `%LOCALAPPDATA%\models\` |

Windows MUST NOT default to `%APPDATA%` (Roaming): roaming
profiles upload `%APPDATA%` to the domain controller / OneDrive,
and a 5 GB blob would replicate to every machine the user signs
into.

### 2. Layout

```
$MODELS_HOME/
├── blobs/
│   └── sha256/
│       ├── ab/                              # 2-char fanout
│       │   └── abcd1234.../
│       │       └── data                     # raw weight bytes
│       └── cd/
│           └── cdef5678.../
│               └── data
├── manifests/
│   ├── gemma-4-e4b.json
│   └── llama-3.1-8b-instruct-q4_k_m.json
└── locks/                                   # advisory flock dir
```

Blob path is `blobs/sha256/<aa>/<full-hash>/data`. The two-
character fanout caps any single directory at ~1 % of its hash
space, which matters on filesystems that scale poorly past tens
of thousands of entries (NTFS legacy 8.3, older ext4).

### 3. Manifest schema

Manifests are small JSON files mapping a human-friendly name to
a SHA-keyed blob. Schema v1:

```json
{
  "schema_version": 1,
  "name": "gemma-4-e4b",
  "format": "gguf",
  "blob": "sha256:30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
  "size_bytes": 5126304928,
  "license": "apache-2.0",
  "source": {
    "registry": "huggingface.co",
    "repo": "unsloth/gemma-4-E4B-it-GGUF",
    "revision": "main",
    "filename": "gemma-4-E4B-it-UD-Q4_K_XL.gguf"
  },
  "produced_by": "inferd/0.1.0-alpha.0",
  "produced_at": "2026-05-18T17:06:10Z"
}
```

Different teams' repacks of the "same" model — Unsloth's
Q4_K_XL, Bartowski's Q4_K_M, an upstream push — get different
manifests but share the blob if and only if their bytes match.
Manifests are cheap; blobs are heavy. The split is the win.

### 4. Read/write contract

**Producer** (the daemon, on first-boot fetch):

1. Acquire `LOCK_EX` on `locks/<name>.lock`.
2. Stream the HTTPS download to
   `blobs/sha256/<aa>/.partial-<hash>/data.tmp`, with a
   running SHA-256.
3. Constant-time compare of computed vs expected SHA
   (THREAT_MODEL F-5).
4. On match: `rename(2)` (POSIX) /
   `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` (Windows) into
   `blobs/sha256/<aa>/<hash>/data`. On mismatch: move into a
   quarantine path under `locks/`, fail loudly (F-6).
5. Write `manifests/<name>.json` last.
6. Release the lock.

**Consumer** (the daemon, on every load including reloads):

1. Read `manifests/<name>.json`.
2. Resolve the named SHA to `blobs/sha256/<aa>/<hash>/data`.
3. `mmap` the blob; never write to it.

Lock-free for readers means N consumers (inferd plus any other
tool honouring the convention) can mmap the same blob
simultaneously without contention.

### 5. Trust boundary

Content addressing is the security model. A malicious file
dropped at an arbitrary path inside `blobs/` is invisible to
inferd — no manifest references it. A swapped blob reference
inside an existing manifest is detected because the blob path
*is* its hash; if the bytes change, the path changes, and the
new blob has to pass the hash check before any consumer
accepts it.

The store is per-user. Cross-user attacks require POSIX
permissions failure, not a flaw in this layout.

### 6. What this ADR is NOT

- **Not a discovery surface.** inferd reads only the manifest
  named in `~/.inferd/config.json`. The daemon does not
  enumerate manifests, list available models, or browse
  blobs. ADR 0006's lean-core posture stands.
- **Not a registry client.** The `source.registry` field is
  diagnostic / migration metadata, not a runtime signal. The
  daemon does not re-resolve manifests against upstream
  registries; ADR 0010's narrow-HTTPS exception applies only
  to the configured `source.url`.
- **Not multi-user.** System-wide stores
  (`/var/lib/models/`, `%PROGRAMDATA%\models\`) are out of
  scope for v1. The proposal sketches them; we don't
  implement them yet.
- **Not garbage-collected.** Orphaned blobs accumulate; the
  daemon doesn't reclaim them. Reclamation is a future
  `inferdctl gc` command tracked separately.

## Consequences

**Why this is the right shape:**

- Other tools adopting the same convention can reuse blobs
  inferd has already fetched (and vice versa). The
  duplication tax goes away for users who opt in across
  their toolchain.
- The CAS layout is cleaner code than a flat dir: blob writes
  are content-addressed (no filename collisions across
  re-quantisations), atomic-rename is a one-liner, dedup is
  free, and the lock dir is a real synchronisation primitive
  rather than implicit.
- Future signed-manifest support (cosign, sigstore) plugs in
  at the manifest layer with no changes to blob storage.
- Migration from existing per-tool silos is straightforward:
  hash the file, write the manifest, optionally hard-link or
  move into the CAS — all reversible.

**What this costs:**

- The first-boot redownload tax for users with an existing
  flat-layout install. Tracked separately as a
  `inferdctl migrate-paths` command per the spec proposal §6;
  the conservative default is to coexist with the legacy path
  read-only for one release before deprecating.
- Two new directories the operator may need to back up
  separately (`blobs/`, `manifests/`). Documented in the
  install README.
- A small advisory-locking surface that has to behave
  correctly under crash. Test coverage is mandatory:
  abandoned `.partial-` directories and stale lock files
  must not block a subsequent daemon start.

**What this explicitly does not change:**

- The wire protocol (`docs/protocol-v1.md`) — unchanged. The
  admin socket's `phase: download` event still carries
  `path`, but that path is now the blob's CAS location
  (`blobs/sha256/<aa>/<hash>/data`) rather than a flat
  filename. Forward-compat: existing clients display the
  string verbatim, which is what the spec required.
- ADR 0006 (lean-core) — unchanged. The store is
  storage-only; no discovery, no enumeration.
- ADR 0010 (HTTPS exception) — unchanged. The exception is
  scoped to `source.url`; the CAS layout is where the bytes
  land, not what authorises the fetch.

## Alternatives considered

- **Keep the flat `models_dir` layout.** Rejected: bakes
  inferd into a per-tool silo that the rest of the local-AI
  ecosystem is moving away from. Cheap to fix today;
  expensive to fix once users have GGUFs scattered across
  per-tool dirs.
- **Adopt Ollama's existing internal layout
  (`blobs/sha256-<hash>` flat, `manifests/<host>/<repo>/<tag>`
  nested).** Rejected: Ollama's manifest path embeds registry
  semantics that are an Ollama-specific implementation
  detail. The cross-tool proposal deliberately picks a
  flatter, registry-agnostic manifest shape.
- **Defer to a future ADR after first ship.** Rejected: pre-
  GA is the cheapest moment to do this. Once users have
  flat-layout installs, every change requires a migration
  story.
- **Implement only the path default
  (`%LOCALAPPDATA%\models`) without the CAS layout.**
  Rejected as a half-measure: the path default is the
  smaller half of the win. CAS dedup, manifest indirection,
  and cross-tool reuse are the load-bearing pieces.

## References

- ADR 0006 — lean-core posture (the store is storage, not
  discovery).
- ADR 0010 — narrow HTTPS exception (what fetches the bytes
  in the first place).
- THREAT_MODEL.md F-5 / F-6 — constant-time SHA compare and
  TOCTOU mitigation that the producer steps rely on.
- Sibling spec: *Shared Local Model Store — Cross-Tool
  Convention Proposal* — the upstream RFC this ADR adopts.
