# 0035. Verify model files once, and actually warm the model at load

- Status: proposed
- Date: 2026-10-30

## Context

Two things happen, or fail to happen, every time the daemon starts:

1. **Every model file is re-hashed.** `loader.rs::load_model` stream-
   hashes the GGUF whenever an expected SHA-256 is configured, which it
   always is for catalog models. With the default install that is the
   5.1 GB generation model, the 0.99 GB projector and the 0.33 GB
   embedding model before anything loads. On a Core Ultra 7 165U, boot
   to `ready` takes 12.5 s; reading and hashing those files is most of
   it (GNU `sha256sum` alone needs 20.1 + 3.1 + 1.2 s for them). THREAT_MODEL F-6 already
   records why this cannot be a security boundary: an attacker able to
   rewrite the user's model file "has write access to the user's model
   file in general, which is already a threat that exceeds what hashing
   can defend against". What the boot-time hash does catch is
   corruption — and the bytes are already verified when they enter the
   store (fetch verifies before the atomic rename; `inferdctl import
   --expect-sha256` verifies before writing).
2. **"Ready" is not warm.** Weights are memory-mapped and no warm-up
   pass runs, so the first request pays for paging the model in and for
   first-use compute-buffer setup. Measured on a Core Ultra 7 165U: a
   28-token / 1-token request right after boot took 4.6 s; the same
   shape warm is well under 1 s. Today the boot hash hides part of this
   by filling the OS file cache as a side effect.

## Decision

1. **Verify on entry, re-verify on change.** The store manifest records,
   next to the digest, a stamp of the verified file: size, last-write
   time and file identity (inode / Windows file ID). At load, a matching
   stamp skips the hash; any mismatch re-hashes with the existing
   constant-time compare (F-5) and refuses on failure, as today.
   `inferdctl verify` forces a full re-hash on demand.
2. **Warm at load.** After the context is created, run one short decode
   (a BOS token, then clear the KV) before reporting `ready`, so weights
   are resident and buffers allocated when the first consumer arrives.
   For accelerators that compile graphs (ADR 0033), this is also where
   the first compilation happens, so its cost lands at boot rather than
   in a user's request.

## Consequences

**Easier:** boot no longer reads ~6.4 GB to rehash unchanged files;
`ready` means a request will be served at full speed.

**Harder / costs:** the manifest gains a stamp field (older manifests
without one fall back to hashing, then gain one); the warm-up adds a
moment to boot that used to be paid by the first request.

**Explicitly NOT changed:** downloads and imports are still verified
against pinned digests before they enter the store; F-5's constant-time
compare; F-6's accepted residual risk.

## References

- THREAT_MODEL F-5, F-6; ADR 0011 (store), ADR 0028 (`inferdctl import`).
- `crates/inferd-engine/src/llamacpp/loader.rs` (`load_model`).
