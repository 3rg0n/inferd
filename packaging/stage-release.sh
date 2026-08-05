#!/usr/bin/env bash
#
# Stage one release archive's contents from an already-built
# `target/<target>/release/` tree.
#
# Usage: packaging/stage-release.sh <stage-dir> <target> <runner-os>
#   <stage-dir>  destination, e.g. staging/inferd-v0.7.0-x86_64-unknown-linux-gnu
#   <target>     rust target triple, used to locate target/<target>/release
#   <runner-os>  Linux | macOS | Windows (GitHub's `runner.os`)
#
# Extracted from `release.yml` when ADR 0028 added a second build pass
# per platform (the airgapped artifact). Both passes call this, which is
# the point: two archives assembled by two copies of a 50-line inline
# script is how their *contents* start to diverge, and "the two artifacts
# are one code path" is the whole premise of that ADR. The binaries
# differ — nothing else does.
#
# Deliberately says nothing about which build profile it is staging. The
# binaries carry that themselves (`--version` reports it, ADR 0028), so
# there is nothing here to get out of step with them.
set -euo pipefail

stage="${1:?stage dir required}"
target="${2:?target triple required}"
runner_os="${3:?runner os required}"

rel="target/${target}/release"
mkdir -p "$stage"

cp README.md LICENSE CHANGELOG.md "$stage/"
# Airgapped install runbook. Shipped in BOTH archives: an operator on a
# disconnected machine cannot open the repo to read it, and `import` is
# useful on a networked host too (ADR 0028).
cp docs/airgapped.md "$stage/"

# ADR 0019: bundle the dl-backends staging dir produced by build.rs
# (`stage_backends_dir`). The whole `backends/` subtree must ship next to
# the daemon binary — libllama has RPATH `$ORIGIN` / `@loader_path` baked
# in, so it loads MODULE libs from `<bin dir>/` and friends. Fail loudly
# if the dir is missing: a release archive without backends would
# silently fall back to "no backend registered" at runtime.
#
# Shared by both build passes. The airgapped pass changes only
# `inferd-daemon`'s own features, so `inferd-engine` is not rebuilt and
# this directory (plus any bundled CUDA redist libs) survives from the
# first pass untouched.
backends_src="$rel/backends"
if [ ! -d "$backends_src" ] || [ -z "$(ls -A "$backends_src" 2>/dev/null)" ]; then
  echo "FAIL: $backends_src missing or empty — dl-backends staging didn't run" >&2
  exit 1
fi
cp -r "$backends_src" "$stage/"

mkdir -p "$stage/packaging"
cp packaging/README.md "$stage/packaging/"

if [ "$runner_os" = "Windows" ]; then
  cp "$rel/inferd-daemon.exe" "$stage/"
  cp "$rel/inferdctl.exe" "$stage/"
  cp "$rel/inferd-http.exe" "$stage/"
  cp packaging/windows/install.ps1 "$stage/packaging/"
  cp packaging/windows/uninstall.ps1 "$stage/packaging/"
  cp packaging/windows/cleanup-legacy-service.ps1 "$stage/packaging/"
else
  cp "$rel/inferd-daemon" "$stage/"
  cp "$rel/inferdctl" "$stage/"
  cp "$rel/inferd-http" "$stage/"
  case "$runner_os" in
    Linux)
      cp packaging/systemd/inferd.service "$stage/packaging/"
      ;;
    macOS)
      cp packaging/launchd/io.inferd.daemon.plist "$stage/packaging/"
      cp packaging/launchd/install-launchagent.sh "$stage/packaging/"
      cp packaging/launchd/uninstall-launchagent.sh "$stage/packaging/"
      chmod +x "$stage/packaging/install-launchagent.sh" \
               "$stage/packaging/uninstall-launchagent.sh"
      ;;
  esac
fi

echo "--- stage root: $stage ---"
ls -la "$stage"
echo "--- backends/ ---"
ls -la "$stage/backends"
