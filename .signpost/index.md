---
okf_version: "0.2"
type: Index
title: Repository map
description: "Structural map of this repository: 135 concepts, 473 relationships."
resource: git://github.com/3rg0n/inferd@359acc8324e8227dab8f418cb1b4a37e0faebeb7
generated: { by: signpost/v0.2.0, at: "2026-10-07" }
---
# Repository map

<!-- signpost:managed:index -->
Start here. What the shape of this repository says, then a line per page naming what is on it.

### How work is done here

- [How work is done here](./practices.md) — what this repository declares about building, testing, gating, and ownership, and what it does not.

### Most connected

The places a wrong assumption propagates furthest, so the places to read first.

- [crates/inferd-engine](./modules/inferd-engine.md) — 58 relationships (17 in, 41 out)
- [crates/inferd-daemon/src](./modules/src-12r0iph.md) — 54 relationships (19 in, 35 out)
- [crates/inferd-engine/src/llamacpp](./modules/llamacpp.md) — 44 relationships (18 in, 26 out)
- [crates/inferd-daemon](./modules/inferd-daemon.md) — 42 relationships (11 in, 31 out)
- [crates/inferd-engine/src/openai_compat](./modules/openai-compat.md) — 38 relationships (14 in, 24 out)

### Structural findings

What the shape of this repository says. Each line is a result — where one reads "none", that is the finding.

- **Import cycles: 1.** The modules in a cycle cannot be understood or changed independently, whatever the directory layout suggests.
  - 2 modules: [crates/inferd-engine/src/llamacpp/chat_template](./modules/chat-template.md), [crates/inferd-engine/src/llamacpp](./modules/llamacpp.md)
- **Cross-cluster edges: 177.** Where a change is most likely to surprise someone: the two sides are maintained as separate concerns and coupled anyway.
  - [crates/inferd-engine/src/bedrock_invoke](./modules/bedrock-invoke.md) → [crates/inferd-engine](./modules/inferd-engine.md) (changes with)
  - [crates/inferd-engine/src/bedrock_invoke](./modules/bedrock-invoke.md) → [crates/inferd-daemon/src](./modules/src-12r0iph.md) (changes with)
  - [crates/inferd-engine/src/bedrock_invoke](./modules/bedrock-invoke.md) → [base64](./references/crates-io-base64.md) (imports)
  - [crates/inferd-engine/src/bedrock_invoke](./modules/bedrock-invoke.md) → [bytes](./references/crates-io-bytes.md) (imports)
  - [crates/inferd-engine/src/bedrock_invoke](./modules/bedrock-invoke.md) → [hmac](./references/crates-io-hmac.md) (imports)
  - [crates/inferd-engine/src/bedrock_invoke](./modules/bedrock-invoke.md) → [serde](./references/crates-io-serde.md) (imports)
  - [crates/inferd-engine/src/bedrock_invoke](./modules/bedrock-invoke.md) → [sha2](./references/crates-io-sha2.md) (imports)
  - [crates/inferd-engine/src/bedrock_invoke](./modules/bedrock-invoke.md) → [tracing](./references/crates-io-tracing.md) (imports)
  - [crates/inferd-engine/src/llamacpp/chat_template](./modules/chat-template.md) → [crates/inferd-engine](./modules/inferd-engine.md) (changes with)
  - [crates/inferd-engine/src/llamacpp/chat_template](./modules/chat-template.md) → [crates/inferd-daemon/src](./modules/src-12r0iph.md) (changes with)
  - [crates/inferd-daemon/tests/common](./modules/common.md) → [crates/inferd-daemon/src](./modules/src-12r0iph.md) (changes with)
  - [crates/inferd-engine/cpp](./modules/cpp.md) → [crates/inferd-engine](./modules/inferd-engine.md) (changes with)
  - [crates/inferd-engine/cpp](./modules/cpp.md) → [crates/inferd-engine/src/llamacpp](./modules/llamacpp.md) (changes with)
  - [crates/inferd-proto/src/embed](./modules/embed.md) → [crates/inferd-proto/src](./modules/src-ymgvev.md) (imports)
  - [clients/go](./modules/go.md) → [crates/inferd-daemon](./modules/inferd-daemon.md) (changes with)
  - [clients/go](./modules/go.md) → [crates/inferd-engine](./modules/inferd-engine.md) (changes with)
  - [clients/go](./modules/go.md) → [packaging](./modules/packaging.md) (changes with)
  - [clients/go](./modules/go.md) → [crates/inferd-daemon/src](./modules/src-12r0iph.md) (changes with)
  - [clients/go](./modules/go.md) → [crates/inferd-http/src](./modules/src-fq5wh.md) (changes with)
  - [crates/inferd-daemon](./modules/inferd-daemon.md) → [clients/go](./modules/go.md) (changes with)
  - and 157 more
- **Disconnected islands: 1.** Concepts linked to each other and to nothing else — most often documents describing code nothing connects them to.
  - 32 concepts: [CI airgapped](./pipelines/ci-airgapped.md), [CI airgapped artifact links no HTTPS stack](./pipelines/ci-airgapped-artifact-links-no-https-stack.md), [CI cargo audit](./pipelines/ci-cargo-audit.md), [CI cargo deny \(licences + MPL containment\)](./pipelines/ci-cargo-deny-licences-mpl-containment.md), [CI default](./pipelines/ci-default.md), [CI dl-backends](./pipelines/ci-dl-backends.md), [CI go-client](./pipelines/ci-go-client.md), [CI llamacpp](./pipelines/ci-llamacpp.md), and 24 more
- **Unconnected concepts: 31.** Nothing links to or from these: dead code, an unreferenced document, or a gap in extraction. Which of the three it is needs a human.
  - [ADR 0001: wire protocol inherited from thlibo](./references/adr-0001-wire-protocol-inherited-from-thlibo.md)
  - [ADR 0002: rust not go](./references/adr-0002-rust-not-go.md)
  - [ADR 0003: subprocess llamafile not ffi](./references/adr-0003-subprocess-llamafile-not-ffi.md)
  - [ADR 0004: mit not apache](./references/adr-0004-mit-not-apache.md)
  - [ADR 0005: libllama ffi not subprocess](./references/adr-0005-libllama-ffi-not-subprocess.md)
  - [ADR 0006: lean core ecosystem extensions](./references/adr-0006-lean-core-ecosystem-extensions.md)
  - [ADR 0007: backend routing and failure semantics](./references/adr-0007-backend-routing-and-failure-semantics.md)
  - [ADR 0008: protocol v1 designed for inferd not derived from thlibo](./references/adr-0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md)
  - [ADR 0009: pre m1 open questions resolved](./references/adr-0009-pre-m1-open-questions-resolved.md)
  - [ADR 0010: narrow https exception for model bootstrap](./references/adr-0010-narrow-https-exception-for-model-bootstrap.md)
  - [ADR 0011: shared content addressable model store](./references/adr-0011-shared-content-addressable-model-store.md)
  - [ADR 0012: one warm model per inferd process](./references/adr-0012-one-warm-model-per-inferd-process.md)
  - [ADR 0013: inferd is the gateway not the pipe](./references/adr-0013-inferd-is-the-gateway-not-the-pipe.md)
  - [ADR 0014: inferd cli is a reference middleware](./references/adr-0014-inferd-cli-is-a-reference-middleware.md)
  - [ADR 0015: v2 wire protocol typed content blocks](./references/adr-0015-v2-wire-protocol-typed-content-blocks.md)
  - [ADR 0016: consumer decodes media before sending](./references/adr-0016-consumer-decodes-media-before-sending.md)
  - [ADR 0017: embeddings on a third socket](./references/adr-0017-embeddings-on-a-third-socket.md)
  - [ADR 0018: cli renamed to inferdctl](./references/adr-0018-cli-renamed-to-inferdctl.md)
  - [ADR 0019: runtime accelerator detection via ggml backend dl](./references/adr-0019-runtime-accelerator-detection-via-ggml-backend-dl.md)
  - [ADR 0020: inferd http bridge is a separate process](./references/adr-0020-inferd-http-bridge-is-a-separate-process.md)
  - and 11 more
- **Merge gates: 14 of 18 CI jobs.** These run on a pull request or on a push to the default branch, so they are the automated checks a change meets. Which of them is *required* is configured on the repository and is not in the tree.
  - [CI airgapped](./pipelines/ci-airgapped.md)
  - [CI airgapped artifact links no HTTPS stack](./pipelines/ci-airgapped-artifact-links-no-https-stack.md)
  - [CI cargo audit](./pipelines/ci-cargo-audit.md)
  - [CI cargo deny \(licences + MPL containment\)](./pipelines/ci-cargo-deny-licences-mpl-containment.md)
  - [CI default](./pipelines/ci-default.md)
  - [CI dl-backends](./pipelines/ci-dl-backends.md)
  - [CI go-client](./pipelines/ci-go-client.md)
  - [CI llamacpp](./pipelines/ci-llamacpp.md)
  - [CI openai](./pipelines/ci-openai.md)
  - [CI security](./pipelines/ci-security.md)
  - [CI systemd-unit smoke \(ubuntu\)](./pipelines/ci-systemd-unit-smoke-ubuntu.md)
  - [Deploy site to GitHub Pages deploy](./pipelines/deploy-site-to-github-pages-deploy.md)
  - [signpost rebuild the bundle](./pipelines/signpost-rebuild-the-bundle.md)
  - [signpost the bundle still describes this tree](./pipelines/signpost-the-bundle-still-describes-this-tree.md)

### Modules

- [crates/inferd-engine/src/bedrock_invoke](./modules/bedrock-invoke.md) — 5 rust files; 6 exported symbols; package crates::inferd-engine::src::bedrock_invoke::adapter.
- [crates/inferd-engine/src/llamacpp/chat_template](./modules/chat-template.md) — 4 rust files; 27 exported symbols; package crates::inferd-engine::src::llamacpp::chat_template::gemma4.
- [crates/inferd-daemon/tests/common](./modules/common.md) — 1 rust file; package crates::inferd-daemon::tests::common.
- [crates/inferd-engine/cpp](./modules/cpp.md) — 2 cpp files; 3 exported symbols.
- [crates/inferd-proto/src/embed](./modules/embed.md) — 3 rust files; 9 exported symbols; package crates::inferd-proto::src::embed.
- [crates/inferd-proto/fuzz/fuzz_targets](./modules/fuzz-targets.md) — 2 rust files; package crates::inferd-proto::fuzz::fuzz_targets::lp_frame_reader.
- [clients/go](./modules/go.md) — 14 go files; 86 exported symbols; package inferd.
- [crates/inferd-daemon](./modules/inferd-daemon.md) — 1 rust file; package crates::inferd-daemon::build.
- [crates/inferd-engine](./modules/inferd-engine.md) — 1 rust file; package crates::inferd-engine::build.
- [packaging/launchd](./modules/launchd.md) — 2 shell files; entrypoint #!.
- [crates/inferd-engine/src/llamacpp](./modules/llamacpp.md) — 7 rust files; 52 exported symbols; package crates::inferd-engine::src::llamacpp::accelerator.
- [crates/inferd-engine/src/openai_compat](./modules/openai-compat.md) — 4 rust files; 5 exported symbols; package crates::inferd-engine::src::openai_compat::adapter.
- [packaging](./modules/packaging.md) — 1 shell file; entrypoint #!.
- [crates/inferd-proto/src/rerank](./modules/rerank.md) — 3 rust files; 11 exported symbols; package crates::inferd-proto::src::rerank.
- [crates/inferd-daemon/src](./modules/src-12r0iph.md) — 21 rust files; 154 exported symbols; entrypoint main; package crates::inferd-daemon::src::admin.
- [crates/inferd-client/src](./modules/src-162lily.md) — 8 rust files; 32 exported symbols; package crates::inferd-client::src::admin.
- [crates/inferd-engine/src](./modules/src-1b7d94r.md) — 5 rust files; 30 exported symbols; package crates::inferd-engine::src::backend.
- [crates/inferd/src](./modules/src-1kxpou4.md) — 1 rust file; entrypoint main; package crates::inferd::src::main.
- [crates/inferd-openai-wire/src](./modules/src-1qcto7x.md) — 1 rust file; 34 exported symbols; package crates::inferd-openai-wire::src::lib.
- [crates/inferd-http/src](./modules/src-fq5wh.md) — 7 rust files; 33 exported symbols; entrypoint main; package crates::inferd-http::src::audio_decode.
- [crates/inferd-proto/src](./modules/src-ymgvev.md) — 3 rust files; 15 exported symbols; package crates::inferd-proto::src::error.
- [crates/inferd-daemon/tests](./modules/tests-1285xfa.md) — 10 rust files; package crates::inferd-daemon::tests::echo.
- [crates/inferd-engine/tests](./modules/tests-1hsi7ps.md) — 10 rust files; package crates::inferd-engine::tests::chat_template_gemma4.
- [crates/inferd/tests](./modules/tests-1qbzbiv.md) — 1 rust file; package crates::inferd::tests::default_endpoints.
- [crates/inferd-proto/tests](./modules/tests-1sec76c.md) — 1 rust file; package crates::inferd-proto::tests::v2_wire.
- [crates/inferd-proto/src/v2](./modules/v2.md) — 5 rust files; 32 exported symbols; package crates::inferd-proto::src::v2::attachment.
- [packaging/validate](./modules/validate.md) — 3 python files; 23 exported symbols; entrypoint __main__; package packaging.validate.bridge.
- [packaging/windows](./modules/windows.md) — 3 powershell files; 1 exported symbol; entrypoint param.

### Pipelines

- [CI airgapped](./pipelines/ci-airgapped.md) — CI job ${{ matrix.os }} / airgapped (--no-default-features) in the CI workflow, 6 steps; runs on a pull request or a default-branch push
- [CI airgapped artifact links no HTTPS stack](./pipelines/ci-airgapped-artifact-links-no-https-stack.md) — CI job airgapped artifact links no HTTPS stack in the CI workflow, 5 steps; runs on a pull request or a default-branch push
- [CI cargo audit](./pipelines/ci-cargo-audit.md) — CI job cargo audit in the CI workflow, 3 steps; runs on a pull request or a default-branch push
- [CI cargo deny \(licences + MPL containment\)](./pipelines/ci-cargo-deny-licences-mpl-containment.md) — CI job cargo deny (licences + MPL containment) in the CI workflow, 3 steps; runs on a pull request or a default-branch push
- [CI default](./pipelines/ci-default.md) — CI job ${{ matrix.os }} / default in the CI workflow, 6 steps; runs on a pull request or a default-branch push
- [CI dl-backends](./pipelines/ci-dl-backends.md) — CI job ${{ matrix.os }} / dl-backends feature in the CI workflow, 8 steps; runs on a pull request or a default-branch push
- [CI go-client](./pipelines/ci-go-client.md) — CI job ${{ matrix.os }} / go client in the CI workflow, 7 steps; runs on a pull request or a default-branch push
- [CI llamacpp](./pipelines/ci-llamacpp.md) — CI job ${{ matrix.os }} / llamacpp feature in the CI workflow, 7 steps; runs on a pull request or a default-branch push
- [CI openai](./pipelines/ci-openai.md) — CI job ${{ matrix.os }} / openai-compat feature in the CI workflow, 5 steps; runs on a pull request or a default-branch push
- [CI security](./pipelines/ci-security.md) — CI job ${{ matrix.os }} / Tier 5 security regression in the CI workflow, 4 steps; runs on a pull request or a default-branch push
- [CI systemd-unit smoke \(ubuntu\)](./pipelines/ci-systemd-unit-smoke-ubuntu.md) — CI job systemd-unit smoke (ubuntu) in the CI workflow, 14 steps; runs on a pull request or a default-branch push
- [Deploy site to GitHub Pages deploy](./pipelines/deploy-site-to-github-pages-deploy.md) — CI job deploy in the Deploy site to GitHub Pages workflow, 4 steps; runs on a pull request or a default-branch push
- [Release build](./pipelines/release-build.md) — CI job ${{ matrix.target }} in the Release workflow, 31 steps
- [Release CycloneDX SBOM](./pipelines/release-cyclonedx-sbom.md) — CI job CycloneDX SBOM in the Release workflow, 6 steps
- [Release Sign + publish release](./pipelines/release-sign-publish-release.md) — CI job Sign + publish release in the Release workflow, 9 steps
- [signpost rebuild the bundle](./pipelines/signpost-rebuild-the-bundle.md) — CI job rebuild the bundle in the signpost workflow, 5 steps; runs on a pull request or a default-branch push
- [signpost the bundle still describes this tree](./pipelines/signpost-the-bundle-still-describes-this-tree.md) — CI job the bundle still describes this tree in the signpost workflow, 3 steps; runs on a pull request or a default-branch push
- [Validate arm64 \(install=work gates\) gates](./pipelines/validate-arm64-install-work-gates-gates.md) — CI job ${{ matrix.name }} in the Validate arm64 (install=work gates) workflow, 21 steps

### Documents

- [ADR 0001: wire protocol inherited from thlibo](./references/adr-0001-wire-protocol-inherited-from-thlibo.md) — Architecture decision (Superseded), 17 rules read from 0001-wire-protocol-inherited-from-thlibo.md.
- [ADR 0002: rust not go](./references/adr-0002-rust-not-go.md) — Architecture decision (Accepted), 24 rules read from 0002-rust-not-go.md.
- [ADR 0003: subprocess llamafile not ffi](./references/adr-0003-subprocess-llamafile-not-ffi.md) — Architecture decision (Superseded), 15 rules read from 0003-subprocess-llamafile-not-ffi.md.
- [ADR 0004: mit not apache](./references/adr-0004-mit-not-apache.md) — Architecture decision (Accepted), 14 rules read from 0004-mit-not-apache.md.
- [ADR 0005: libllama ffi not subprocess](./references/adr-0005-libllama-ffi-not-subprocess.md) — Architecture decision (Accepted), 37 rules read from 0005-libllama-ffi-not-subprocess.md.
- [ADR 0006: lean core ecosystem extensions](./references/adr-0006-lean-core-ecosystem-extensions.md) — Architecture decision (Accepted), 48 rules read from 0006-lean-core-ecosystem-extensions.md.
- [ADR 0007: backend routing and failure semantics](./references/adr-0007-backend-routing-and-failure-semantics.md) — Architecture decision (Accepted), 42 rules read from 0007-backend-routing-and-failure-semantics.md.
- [ADR 0008: protocol v1 designed for inferd not derived from thlibo](./references/adr-0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md) — Architecture decision (Accepted), 30 rules read from 0008-protocol-v1-designed-for-inferd-not-derived-from-thlibo.md.
- [ADR 0009: pre m1 open questions resolved](./references/adr-0009-pre-m1-open-questions-resolved.md) — Architecture decision (Accepted), 37 rules read from 0009-pre-m1-open-questions-resolved.md.
- [ADR 0010: narrow https exception for model bootstrap](./references/adr-0010-narrow-https-exception-for-model-bootstrap.md) — Architecture decision (Accepted), 48 rules read from 0010-narrow-https-exception-for-model-bootstrap.md.
- [ADR 0011: shared content addressable model store](./references/adr-0011-shared-content-addressable-model-store.md) — Architecture decision (Accepted), 58 rules read from 0011-shared-content-addressable-model-store.md.
- [ADR 0012: one warm model per inferd process](./references/adr-0012-one-warm-model-per-inferd-process.md) — Architecture decision (Accepted), 32 rules read from 0012-one-warm-model-per-inferd-process.md.
- [ADR 0013: inferd is the gateway not the pipe](./references/adr-0013-inferd-is-the-gateway-not-the-pipe.md) — Architecture decision (Accepted), 50 rules read from 0013-inferd-is-the-gateway-not-the-pipe.md.
- [ADR 0014: inferd cli is a reference middleware](./references/adr-0014-inferd-cli-is-a-reference-middleware.md) — Architecture decision (Superseded), 30 rules read from 0014-inferd-cli-is-a-reference-middleware.md.
- [ADR 0015: v2 wire protocol typed content blocks](./references/adr-0015-v2-wire-protocol-typed-content-blocks.md) — Architecture decision (Accepted), 80 rules read from 0015-v2-wire-protocol-typed-content-blocks.md.
- [ADR 0016: consumer decodes media before sending](./references/adr-0016-consumer-decodes-media-before-sending.md) — Architecture decision (Accepted), 34 rules read from 0016-consumer-decodes-media-before-sending.md.
- [ADR 0017: embeddings on a third socket](./references/adr-0017-embeddings-on-a-third-socket.md) — Architecture decision (Accepted), 73 rules read from 0017-embeddings-on-a-third-socket.md.
- [ADR 0018: cli renamed to inferdctl](./references/adr-0018-cli-renamed-to-inferdctl.md) — Architecture decision (Accepted), 28 rules read from 0018-cli-renamed-to-inferdctl.md.
- [ADR 0019: runtime accelerator detection via ggml backend dl](./references/adr-0019-runtime-accelerator-detection-via-ggml-backend-dl.md) — Architecture decision (Accepted), 51 rules read from 0019-runtime-accelerator-detection-via-ggml-backend-dl.md.
- [ADR 0020: inferd http bridge is a separate process](./references/adr-0020-inferd-http-bridge-is-a-separate-process.md) — Architecture decision (Accepted), 25 rules read from 0020-inferd-http-bridge-is-a-separate-process.md.
- [ADR 0021: unified v2 wire length prefixed blob framing](./references/adr-0021-unified-v2-wire-length-prefixed-blob-framing.md) — Architecture decision (Accepted), 46 rules read from 0021-unified-v2-wire-length-prefixed-blob-framing.md.
- [ADR 0022: no inbound network listener deprecate loopback tcp](./references/adr-0022-no-inbound-network-listener-deprecate-loopback-tcp.md) — Architecture decision (Accepted), 26 rules read from 0022-no-inbound-network-listener-deprecate-loopback-tcp.md.
- [ADR 0023: boot time model auto selection by accelerator memory](./references/adr-0023-boot-time-model-auto-selection-by-accelerator-memory.md) — Architecture decision (Accepted), 33 rules read from 0023-boot-time-model-auto-selection-by-accelerator-memory.md.
- [ADR 0024: wsl relay for containerized middleware](./references/adr-0024-wsl-relay-for-containerized-middleware.md) — Architecture decision (Accepted), 29 rules read from 0024-wsl-relay-for-containerized-middleware.md.
- [ADR 0025: bridge decodes and resamples audio](./references/adr-0025-bridge-decodes-and-resamples-audio.md) — Architecture decision (Accepted), 49 rules read from 0025-bridge-decodes-and-resamples-audio.md.
- [ADR 0026: chat renderer registry per model family](./references/adr-0026-chat-renderer-registry-per-model-family.md) — Architecture decision (Accepted), 47 rules read from 0026-chat-renderer-registry-per-model-family.md.
- [ADR 0027: reranking on a fourth socket](./references/adr-0027-reranking-on-a-fourth-socket.md) — Architecture decision (Accepted), 93 rules read from 0027-reranking-on-a-fourth-socket.md.
- [ADR 0028: airgapped build profile](./references/adr-0028-airgapped-build-profile.md) — Architecture decision (Accepted), 71 rules read from 0028-airgapped-build-profile.md.
- [ADR 0029: tool choice is enforced by grammar not advertised](./references/adr-0029-tool-choice-is-enforced-by-grammar-not-advertised.md) — Architecture decision (Accepted), 49 rules read from 0029-tool-choice-is-enforced-by-grammar-not-advertised.md.
- [CLAUDE.md](./references/claude-md.md) — Stated constraints, 98 rules read from CLAUDE.md.
- [README.md](./references/readme-md.md) — Architecture decision, 7 rules read from README.md.

### External dependencies

- [ggml](./references/cmake-ggml.md) — cmake dependency ggml
- [llama](./references/cmake-llama.md) — cmake dependency llama
- [Threads](./references/cmake-threads.md) — cmake dependency Threads
- [anyhow](./references/crates-io-anyhow.md) — crates.io dependency anyhow (1, workspace)
- [async-stream](./references/crates-io-async-stream.md) — crates.io dependency async-stream (0.3)
- [async-trait](./references/crates-io-async-trait.md) — crates.io dependency async-trait (0.1, workspace)
- [axum](./references/crates-io-axum.md) — crates.io dependency axum (0.7)
- [base64](./references/crates-io-base64.md) — crates.io dependency base64 (0.23)
- [bindgen](./references/crates-io-bindgen.md) — crates.io dependency bindgen (0.71)
- [bytes](./references/crates-io-bytes.md) — crates.io dependency bytes (1, workspace)
- [chrono](./references/crates-io-chrono.md) — crates.io dependency chrono (0.4)
- [clap](./references/crates-io-clap.md) — crates.io dependency clap (4, workspace)
- [cmake](./references/crates-io-cmake.md) — crates.io dependency cmake (0.1)
- [eventsource-stream](./references/crates-io-eventsource-stream.md) — crates.io dependency eventsource-stream (0.2)
- [futures-core](./references/crates-io-futures-core.md) — crates.io dependency futures-core (0.3)
- [futures-util](./references/crates-io-futures-util.md) — crates.io dependency futures-util (0.3)
- [hex](./references/crates-io-hex.md) — crates.io dependency hex (0.4)
- [hmac](./references/crates-io-hmac.md) — crates.io dependency hmac (0.13)
- [image](./references/crates-io-image.md) — crates.io dependency image (0.25)
- [inferd-client](./references/crates-io-inferd-client.md) — crates.io dependency inferd-client (=0.8.0)
- [inferd-daemon](./references/crates-io-inferd-daemon.md) — crates.io dependency inferd-daemon (=0.8.0)
- [inferd-engine](./references/crates-io-inferd-engine.md) — crates.io dependency inferd-engine (=0.8.0)
- [inferd-openai-wire](./references/crates-io-inferd-openai-wire.md) — crates.io dependency inferd-openai-wire (=0.8.0)
- [inferd-proto](./references/crates-io-inferd-proto.md) — crates.io dependency inferd-proto (=0.8.0)
- [libfuzzer-sys](./references/crates-io-libfuzzer-sys.md) — crates.io dependency libfuzzer-sys (0.4)
- [nix](./references/crates-io-nix.md) — crates.io dependency nix (0.27)
- [regex](./references/crates-io-regex.md) — crates.io dependency regex (1)
- [reqwest](./references/crates-io-reqwest.md) — crates.io dependency reqwest (0.12)
- [rubato](./references/crates-io-rubato.md) — crates.io dependency rubato (4)
- [serde](./references/crates-io-serde.md) — crates.io dependency serde (1, workspace)
- [serde_json](./references/crates-io-serde-json.md) — crates.io dependency serde_json (1, workspace)
- [sha2](./references/crates-io-sha2.md) — crates.io dependency sha2 (0.11, workspace)
- [subtle](./references/crates-io-subtle.md) — crates.io dependency subtle (2, workspace)
- [symphonia](./references/crates-io-symphonia.md) — crates.io dependency symphonia (0.6)
- [tempfile](./references/crates-io-tempfile.md) — crates.io dependency tempfile (3)
- [thiserror](./references/crates-io-thiserror.md) — crates.io dependency thiserror (2, workspace)
- [tokio](./references/crates-io-tokio.md) — crates.io dependency tokio (1, workspace)
- [tokio-stream](./references/crates-io-tokio-stream.md) — crates.io dependency tokio-stream (0.1)
- [tracing](./references/crates-io-tracing.md) — crates.io dependency tracing (0.1, workspace)
- [tracing-subscriber](./references/crates-io-tracing-subscriber.md) — crates.io dependency tracing-subscriber (0.3, workspace)
- [ureq](./references/crates-io-ureq.md) — crates.io dependency ureq (2)
- [url](./references/crates-io-url.md) — crates.io dependency url (2)
- [windows-sys](./references/crates-io-windows-sys.md) — crates.io dependency windows-sys (0.52)
- [wiremock](./references/crates-io-wiremock.md) — crates.io dependency wiremock (0.6)
- [actions/checkout](./references/github-actions-actions-checkout.md) — github-actions dependency actions/checkout (3d3c42e5aac5ba805825da76410c181273ba90b1, de0fac2e4500dabe0009e67214ff5f5447ce83dd, v4, v6)
- [actions/configure-pages](./references/github-actions-actions-configure-pages.md) — github-actions dependency actions/configure-pages (v5)
- [actions/deploy-pages](./references/github-actions-actions-deploy-pages.md) — github-actions dependency actions/deploy-pages (v4)
- [actions/download-artifact](./references/github-actions-actions-download-artifact.md) — github-actions dependency actions/download-artifact (3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c)
- [actions/setup-go](./references/github-actions-actions-setup-go.md) — github-actions dependency actions/setup-go (v6)
- [actions/upload-artifact](./references/github-actions-actions-upload-artifact.md) — github-actions dependency actions/upload-artifact (043fb46d1a93c77aae656e7c1c64a875d1fc6a0a)
- [actions/upload-pages-artifact](./references/github-actions-actions-upload-pages-artifact.md) — github-actions dependency actions/upload-pages-artifact (v3)
- [dtolnay/rust-toolchain](./references/github-actions-dtolnay-rust-toolchain.md) — github-actions dependency dtolnay/rust-toolchain (29eef336d9b2848a0b548edc03f92a220660cdb8, stable)
- [ilammy/msvc-dev-cmd](./references/github-actions-ilammy-msvc-dev-cmd.md) — github-actions dependency ilammy/msvc-dev-cmd (0b201ec74fa43914dc39ae48a89fd1d8cb592756)
- [Jimver/cuda-toolkit](./references/github-actions-jimver-cuda-toolkit.md) — github-actions dependency Jimver/cuda-toolkit (3d45d157f327c09c04b50ee6ccdea2d9d017ec76)
- [KyleMayes/install-llvm-action](./references/github-actions-kylemayes-install-llvm-action.md) — github-actions dependency KyleMayes/install-llvm-action (a7a1a882e2d06ebe05d5bb97c3e1f8c984ae96fc)
- [sigstore/cosign-installer](./references/github-actions-sigstore-cosign-installer.md) — github-actions dependency sigstore/cosign-installer (398d4b0eeef1380460a10c8013a76f728fb906ac)
- [softprops/action-gh-release](./references/github-actions-softprops-action-gh-release.md) — github-actions dependency softprops/action-gh-release (3bb12739c298aeb8a4eeaf626c5b8d85266b0e65)
- [Swatinem/rust-cache](./references/github-actions-swatinem-rust-cache.md) — github-actions dependency Swatinem/rust-cache (e18b497796c12c097a38f9edb9d0641fb99eee32, v2)
<!-- /signpost:managed:index -->

## Notes

_Anything written here is yours. signpost rewrites only the regions between its managed markers, and never this section._
