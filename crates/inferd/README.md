# inferdctl

The single CLI for [inferd](https://github.com/3rg0n/inferd), in the
`gh` / `kubectl` shape — one binary, many subcommands. Distinct from
`inferd-daemon` (the long-running service); `inferdctl` is what
operators and consumers run from a shell.

`inferdctl` is **reference middleware, not a privileged surface**
([ADR 0014](https://github.com/3rg0n/inferd/blob/main/docs/adr/0014-inferd-cli-is-a-reference-middleware.md)):
it talks to the daemon over the same `inferd-client` library every
other consumer uses. The crate directory is `crates/inferd/`, but the
package and binary are both `inferdctl` (renamed per
[ADR 0018](https://github.com/3rg0n/inferd/blob/main/docs/adr/0018-cli-renamed-to-inferdctl.md)).

## Install

Ships in every release tarball alongside `inferd-daemon`
(<https://github.com/3rg0n/inferd/releases>), or build from source:

```sh
cargo install inferdctl
```

## Subcommands

| Command | What it does |
|---|---|
| `inferdctl status` | One-shot admin snapshot as JSON: one `capabilities` line per registered backend, then the current lifecycle state on the last line. Exits 0 on `ready`, non-zero otherwise — useful in shell scripts. |
| `inferdctl watch` | Stream admin lifecycle events forever. Useful during the first-boot model download. |
| `inferdctl pull` | Read `~/.inferd/config.json`, fetch the configured model into the CAS store (`$MODELS_HOME/blobs/sha256/<aa>/<hash>/data`), verify SHA-256 with a constant-time compare, write the manifest. Operates directly on the store — does not require a running daemon. |
| `inferdctl doctor` | Diagnose connectivity and install state: config, manifest, admin socket, backend readiness, accelerator, and `wire_version`. Prints a "what's there / what's missing" punch list. |

## Global flags

| Flag | Env | Default |
|---|---|---|
| `--config <path>` | `INFERD_CONFIG` | `~/.inferd/config.json` |
| `--admin-addr <path>` | `INFERD_ADMIN_ADDR` | platform default admin socket |

## Example

```console
$ inferdctl status
{"accelerator":"cuda","backend":"embeddinggemma-300m","embed":true,"id":"admin","status":"capabilities","type":"status","v2":true}
{"accelerator":"cuda","audio":true,"backend":"gemma-4-e4b","id":"admin","status":"capabilities","type":"status","v2":true,"vision":true}
{"id":"admin","status":"ready","type":"status"}

$ inferdctl status | tail -1        # just the lifecycle line
{"id":"admin","status":"ready","type":"status"}

$ inferdctl doctor
[ ok ] config:    loaded ~/.inferd/config.json (auto_pull=true)
[ ok ] manifest:  gemma-4-e4b · embeddinggemma-300m
[ ok ] admin:     ready
[ ok ] backend:   llamacpp accelerator=metal gpu_layers=99 embed=true wire_version=1
[ ok ] device:    Apple M2 Pro vram=16.0 GiB
```

## License

MIT. See `LICENSE` in the upstream repo.
