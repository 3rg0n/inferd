# The airgapped build

inferd ships two archives per platform on every release. They are the
same crates at the same tag; only the build flags differ.

| Archive | HTTPS client | How models get in |
|---|---|---|
| `inferd-<ver>-<target>` | linked (ADR 0010) | auto-pulled on first boot, or `inferdctl pull` |
| `inferd-airgapped-<ver>-<target>` | **not linked** | `inferdctl import` only |

Pick the airgapped archive when the deployment has no egress, or when a
reviewer needs the no-network property to be a fact about the binary
rather than a claim about its configuration. Everything else — the wire
protocol, the CAS store layout, SHA-256 verification, accelerator
selection, `inferd-http` — is identical.

Which one is installed is a question you can ask the process:

```sh
$ inferd-daemon --version
inferd-daemon 0.6.1
build profile: airgapped (model-fetch off — no HTTPS client linked; load models with `inferdctl import`)
```

The daemon also logs `build=airgapped` in its first activity-log line.
`inferdctl --version` reports the same thing (both binaries ship in the
same archive and are built together).

## What "no HTTPS client" means, exactly

The airgapped build is compiled with `--no-default-features`, which
turns off the default-on `model-fetch` feature. That drops the `ureq`
dependency, and with it `rustls`, `ring`, `webpki-roots`, and the
native-certificate store. They are **not in the binary** — not present
but unreachable, not gated behind a runtime check. Verify it yourself
against any tag, without reading inferd's source:

```sh
cargo tree -p inferd-daemon --no-default-features --features dl-backends --color never \
  | grep -E '\b(ureq|reqwest|rustls|ring|native-tls|openssl|webpki-roots)\b'
# no output = no HTTPS client stack linked
```

`--color never` matters if you have `CARGO_TERM_COLOR=always` set: cargo
then wraps every line in ANSI escapes, and a grep that anchors on the
crate name silently matches nothing — which reads as a pass.

CI runs exactly that assertion (`.github/workflows/ci.yml`, the
`no-network-deps` job) and fails the build if anything matches, in both
the daemon and `inferdctl`. See [ADR 0028](adr/0028-airgapped-build-profile.md)
for why this had to be feature *subtraction* — an additive `airgapped`
feature cannot remove a dependency, so it would have compiled out the
call sites while leaving the TLS stack linked.

Two things are worth stating plainly, because they are load-bearing:

- **The cloud adapters are absent from both archives.** The
  `openai-compat` and `bedrock-invoke` backends are off by default and
  neither release build enables them. The `cargo tree` assertion covers
  `reqwest`/`hyper` too, so re-enabling one would fail CI rather than
  ship quietly.
- **`inferd-http` is in the airgapped archive, and that is fine.** It is
  an *inbound* localhost listener (ADR 0020) with no HTTP client — it
  cannot originate a request. CI asserts specifically that `hyper`'s
  `client` feature is not enabled anywhere in the workspace, rather than
  banning the crate by name.

## Installing

Install exactly as the networked archive (README §Install) — same
binaries, same `backends/`, same installer scripts. The one difference
is the first boot: the daemon writes `~/.inferd/config.json` and then
**cannot fetch the models it names**. That is expected, and it is the
useful order of operations, because the config it just wrote tells you
what to import.

```
inferd-daemon: model "gemma-4-e4b" is not in the model store and this is
an airgapped build (no model-fetch feature); import it with
`inferdctl import --name gemma-4-e4b <path.gguf>`
```

## Importing models

`inferdctl import` is the offline counterpart to `pull`. It ships in
**both** archives — a subcommand present in only one artifact is a
subcommand nobody tests, and importing a hand-downloaded GGUF is useful
on a networked machine too.

```sh
inferdctl import --name gemma-4-e4b /media/usb/gemma-4-E4B-it-UD-Q4_K_XL.gguf
```

It hashes the file while copying it into
`<store>/blobs/sha256/<aa>/<full-hash>/data` via the same
partial-then-rename producer flow `pull` uses, re-reads the landed bytes
to confirm the copy, and writes `<store>/manifests/<name>.json` last.
The source file is never moved or modified — it may be on removable
media you carried in. The store path is derived from the digest, so you
do not choose it.

Pass the vendor's published digest and it is checked with a
constant-time compare before anything is written:

```sh
inferdctl import --name gemma-4-e4b \
  --expect-sha256 30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36 \
  /media/usb/gemma-4-E4B-it-UD-Q4_K_XL.gguf
```

On mismatch nothing is imported and the file is left alone. Without
`--expect-sha256` the import still succeeds — it just has nothing to
check against, which is why you should pass it when you have it.

Import is idempotent: re-importing the same bytes finds the blob already
present and only refreshes the manifest.

### What to import for a default install

The first-boot `config.json` names each model, and this is the list a
stock install expects. Digests are in that file; the URLs are there too,
so you can fetch the files on a connected machine and carry them in.

| `--name` | What it is | Needed for |
|---|---|---|
| `gemma-4-e4b` | Gemma 4 E4B instruction-tuned GGUF | generation |
| `gemma-4-e4b-mmproj` | multimodal projector (F16) | vision + audio attachments |
| `embeddinggemma-300m` | EmbeddingGemma 300M | the embeddings socket |

Import only what you need. A generation-only deployment can delete the
embed backend and the `mmproj` block from `config.json` instead of
importing them — the daemon binds a socket only when the active backend
advertises that capability, so an absent projector means no vision, not
a failed boot.

## Configuring an imported model

An imported model has no URL. Set `source_url` to the empty string and
the daemon resolves the model from the store's manifest and skips
fetching entirely:

```json
{
  "models_home": "/srv/models",
  "backends": [
    {
      "kind": "llamacpp",
      "name": "gemma-4-e4b",
      "model": {
        "name": "gemma-4-e4b",
        "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
        "source_url": ""
      },
      "n_ctx": 8192,
      "n_gpu_layers": 999
    }
  ]
}
```

`sha256` stays required and is still verified on every boot — the
constant-time re-hash of the blob is the check a hardened deployment
cares about most, and it is untouched by this build. A blob that fails
it is quarantined, exactly as in the networked build.

Editing the first-boot config in place is the shortest path: clear each
`source_url`, leave everything else alone.

The empty `source_url` is valid in the networked build too. If the model
is missing there, you get `no source_url and no manifest exists` — which
names the actual mistake (nothing imported) rather than a network error.

## Running

Nothing changes. Start the daemon the way the installer set up, then:

```sh
inferdctl status
inferdctl doctor
```

`status` prints one `capabilities` line per registered backend and then
the lifecycle state, and exits 0 on `ready`; if the model names match
what you imported, install=work is satisfied. The daemon makes no
outbound connection at any point — it has nothing to make one with.

## Updating

Download the next release's airgapped archive, reinstall over the top,
and leave the model store alone. Blobs are content-addressed, so an
upgrade that keeps the same models re-uses them with no re-import. Only
a model change needs a new `import`.

## References

- [ADR 0028](adr/0028-airgapped-build-profile.md) — why this is a
  default-on feature turned off, and what is deliberately *not* gated.
- [ADR 0010](adr/0010-narrow-https-exception-for-model-bootstrap.md) —
  the HTTPS exception this build declines. Unchanged for the networked
  build.
- [ADR 0011](adr/0011-shared-content-addressable-model-store.md) — the
  CAS store layout `import` writes.
- `docs/RELEASING.md` — the release asset list, including both archives.
