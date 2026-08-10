//! Boot-time model auto-selection by accelerator memory (ADR 0023).
//!
//! When `model_autoselect: "auto"` is set, the daemon picks which
//! Gemma 4 generation variant to warm based on the chosen accelerator's
//! **total** memory (stable), and uses **free** memory only for a
//! pre-load fit check. The embed model co-locates on the accelerator
//! unless memory is tight, in which case it falls back to CPU (per-
//! backend `n_gpu_layers = 0`) rather than failing to load.
//!
//! The decision logic here is pure — it takes memory numbers as input
//! so it is unit-testable without a GPU. The daemon supplies the real
//! numbers from `inferd_engine::llamacpp::query_device_memory_for_kind`
//! at boot (see `main.rs`).

use crate::config_file::{BackendEntry, ConfigFile, LlamacppEntry, ModelAutoselect, ModelConfig};

const GIB: u64 = 1024 * 1024 * 1024;

/// Which generation tier auto-select picked, for logging/tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Gemma 4 E4B — the smaller/faster variant (default tier).
    E4b,
    /// Gemma 4 12B — the larger dense variant, picked on high-VRAM GPUs.
    B12,
}

impl Tier {
    /// Backend name for this tier, matching the synthesised entry name.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::E4b => "gemma-4-e4b",
            Tier::B12 => "gemma-4-12b",
        }
    }
}

/// Pick the generation tier from **total** accelerator memory.
///
/// `total_vram_bytes = None` (no accelerator memory reported — e.g. CPU
/// fallback or an unknown-memory backend) always yields E4B: without a
/// GPU big enough to be sure, the safe pick is the smaller model.
pub fn pick_tier(total_vram_bytes: Option<u64>, min_vram_gib: u32) -> Tier {
    match total_vram_bytes {
        Some(total) if total >= (min_vram_gib as u64) * GIB => Tier::B12,
        _ => Tier::E4b,
    }
}

/// Estimated VRAM (bytes) a llamacpp gen backend needs on the
/// accelerator, at the given context. Deliberately conservative — used
/// for the pre-load fit check and the embed-co-location decision, not
/// for exact accounting. Figures come from
/// `docs/benchmarks/gemma4-e4b-vs-12b.md` (RTX 5080, UD-Q4_K_XL):
/// E4B@8k ≈ 5.7 GB weights+KV over idle; 12B@8k ≈ 11.8 GB (14.6 GB used
/// minus ~2.8 GB desktop floor). KV cache grows ~linearly with n_ctx, so
/// scale the 8k baseline. mmproj (~0.2–1 GB) added when present.
pub fn estimate_gen_vram_bytes(tier: Tier, n_ctx: u32, has_mmproj: bool) -> u64 {
    // (weights+overhead at 8k, per-token KV growth beyond/below 8k).
    let (base_8k_gib, kv_per_1k_ctx_mib): (f64, f64) = match tier {
        Tier::E4b => (5.7, 60.0),
        Tier::B12 => (11.8, 150.0),
    };
    let base = (base_8k_gib * GIB as f64) as u64;
    // Scale KV by the delta from the 8k baseline (can be negative for
    // smaller contexts).
    let ctx_delta_1k = (n_ctx as f64 - 8192.0) / 1024.0;
    let kv_delta = (ctx_delta_1k * kv_per_1k_ctx_mib * 1024.0 * 1024.0) as i64;
    let mmproj = if has_mmproj { GIB } else { 0 };
    (base as i64 + kv_delta).max(0) as u64 + mmproj
}

/// Embed model (embeddinggemma-300m, Q8_0) VRAM estimate on GPU.
pub fn estimate_embed_vram_bytes() -> u64 {
    // ~330 MB weights + small KV at 2048 ctx + overhead.
    (0.7 * GIB as f64) as u64
}

/// The outcome of applying auto-select to a config.
#[derive(Debug)]
pub struct AutoselectOutcome {
    /// The generation tier that was selected.
    pub tier: Tier,
    /// True when the embed backend was pushed to CPU because gen + embed
    /// would not both fit the accelerator with headroom.
    pub embed_forced_cpu: bool,
}

/// Reserve this much accelerator memory free after loading, for compute
/// buffers + desktop overhead the estimates don't capture.
const HEADROOM_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB

/// Build the default (pinned) llamacpp generation entry for a tier.
/// Mirrors the first-boot defaults; SHAs match
/// `docs/benchmarks/gemma4-e4b-vs-12b.md` and the HuggingFace repos.
///
/// `source_url`s pin an immutable repo revision, never `resolve/main/` —
/// see the note in `config_file.rs`: upstream re-quantised both text GGUFs
/// on 2026-07-17 and the mutable URL turned a fresh install into a
/// SHA-mismatch restart loop.
fn default_gen_entry(tier: Tier, n_ctx: u32, n_gpu_layers: i32) -> LlamacppEntry {
    match tier {
        Tier::E4b => LlamacppEntry {
            name: "gemma-4-e4b".into(),
            model: ModelConfig {
                name: "gemma-4-e4b".into(),
                sha256: "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36".into(),
                size_bytes: Some(5_126_304_928),
                source_url: "https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF/resolve/\
                     0720adb23527c2cd5ea01d1db067cd960327fdac/gemma-4-E4B-it-UD-Q4_K_XL.gguf"
                    .into(),
                license: Some("gemma".into()),
            },
            mmproj: Some(ModelConfig {
                name: "gemma-4-e4b-mmproj".into(),
                sha256: "ddf46c21d7078e95338cfc22306b19b276a29a5ad089023449dd54d4b6170a51".into(),
                size_bytes: Some(990_372_672),
                source_url: "https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF/resolve/\
                     0720adb23527c2cd5ea01d1db067cd960327fdac/mmproj-F16.gguf"
                    .into(),
                license: Some("gemma".into()),
            }),
            mmproj_image_max_tokens: None,
            n_ctx,
            n_gpu_layers,
            embed: false,
            embed_pooling: None,
            embed_n_ctx: 2048,
            // Gemma 4 has no classification head — rerank needs a
            // cross-encoder (ADR 0027).
            rerank: false,
            rerank_n_ctx: 2048,
            // Detected from GGUF metadata (ADR 0026).
            chat_template: None,
        },
        Tier::B12 => LlamacppEntry {
            name: "gemma-4-12b".into(),
            model: ModelConfig {
                name: "gemma-4-12b".into(),
                sha256: "ee33ab5be8e07aca1c269fc645eaed5f3298e089d52db29415839d8f29957020".into(),
                size_bytes: Some(7_366_421_920),
                source_url: "https://huggingface.co/unsloth/gemma-4-12b-it-GGUF/resolve/\
                     d997c805aafe035a8024f961c6e1afd6b30d79a5/gemma-4-12b-it-UD-Q4_K_XL.gguf"
                    .into(),
                license: Some("gemma".into()),
            },
            mmproj: Some(ModelConfig {
                name: "gemma-4-12b-mmproj".into(),
                sha256: "91f086971e56d7a7d8d39e271873fccdb49541bd259d6e02c401a4f1cb7a219e".into(),
                size_bytes: Some(175_115_840),
                source_url: "https://huggingface.co/unsloth/gemma-4-12b-it-GGUF/resolve/\
                     d997c805aafe035a8024f961c6e1afd6b30d79a5/mmproj-F16.gguf"
                    .into(),
                license: Some("gemma".into()),
            }),
            mmproj_image_max_tokens: None,
            n_ctx,
            n_gpu_layers,
            embed: false,
            embed_pooling: None,
            embed_n_ctx: 2048,
            // Gemma 4 has no classification head — rerank needs a
            // cross-encoder (ADR 0027).
            rerank: false,
            rerank_n_ctx: 2048,
            // Detected from GGUF metadata (ADR 0026).
            chat_template: None,
        },
    }
}

/// True if the entry is the generation model — a llamacpp entry serving
/// neither embed nor rerank. Both of the non-generation capabilities have
/// to be excluded here: a rerank-only cross-encoder is not a generation
/// backend, and treating it as one would read as an operator-pinned
/// generation model and suppress auto-select entirely (ADR 0027).
fn is_gen_llamacpp(e: &BackendEntry) -> bool {
    matches!(e, BackendEntry::Llamacpp(l) if !l.embed && !l.rerank)
}

/// Apply ADR 0023 auto-selection to a config in place.
///
/// - No-op (returns `None`) when `model_autoselect != Auto`, or when the
///   operator has explicitly pinned a generation llamacpp backend
///   (explicit config always wins — power-user override).
/// - Otherwise picks the tier from `total_vram_bytes`, synthesises the
///   generation backend from defaults if absent, and decides embed
///   placement from `free_vram_bytes` vs. the estimated footprint.
///
/// Pure w.r.t. hardware: all memory numbers are inputs.
pub fn apply(
    cfg: &mut ConfigFile,
    total_vram_bytes: Option<u64>,
    free_vram_bytes: Option<u64>,
) -> Option<AutoselectOutcome> {
    if cfg.model_autoselect != ModelAutoselect::Auto {
        return None;
    }

    let backends = cfg.backends.get_or_insert_with(Vec::new);

    // Power-user override: an explicitly-pinned generation backend wins.
    if backends.iter().any(is_gen_llamacpp) {
        return None;
    }

    let tier = pick_tier(total_vram_bytes, cfg.model_autoselect_min_vram_gib);

    // Default gpu layers: offload all on GPU (-1); the engine clamps to
    // CPU when no accelerator is present.
    let gen_entry = default_gen_entry(tier, cfg.n_ctx.max(default_n_ctx_val()), -1);
    let has_mmproj = gen_entry.mmproj.is_some();
    let gen_ctx = gen_entry.n_ctx;

    // Decide embed placement: does gen + embed fit free VRAM with
    // headroom? If free is unknown, keep embed on GPU (best effort — the
    // pre-load fit check in main.rs is the backstop).
    let gen_est = estimate_gen_vram_bytes(tier, gen_ctx, has_mmproj);
    let embed_est = estimate_embed_vram_bytes();
    let embed_forced_cpu = match free_vram_bytes {
        Some(free) => free < gen_est + embed_est + HEADROOM_BYTES,
        None => false,
    };

    // Rebuild the backend list: synthesised gen first, then any existing
    // embed backend (with n_gpu_layers forced to 0 if memory-constrained),
    // then any non-llamacpp backends (openai/bedrock) preserved in order.
    let mut new_backends: Vec<BackendEntry> = Vec::with_capacity(backends.len() + 1);
    new_backends.push(BackendEntry::Llamacpp(gen_entry));

    let mut had_embed = false;
    for e in backends.drain(..) {
        match e {
            BackendEntry::Llamacpp(mut l) if l.embed => {
                had_embed = true;
                if embed_forced_cpu {
                    l.n_gpu_layers = 0;
                }
                new_backends.push(BackendEntry::Llamacpp(l));
            }
            // A pre-existing non-embed llamacpp can't occur here (we
            // returned early if one was pinned), but keep it defensively.
            other => new_backends.push(other),
        }
    }

    // No embed backend listed → synthesise the default embed backend so
    // "auto" gives a working embed socket out of the box.
    if !had_embed {
        let mut embed = default_embed_entry();
        if embed_forced_cpu {
            embed.n_gpu_layers = 0;
        }
        new_backends.push(BackendEntry::Llamacpp(embed));
    }

    *backends = new_backends;

    Some(AutoselectOutcome {
        tier,
        embed_forced_cpu,
    })
}

fn default_n_ctx_val() -> u32 {
    8192
}

/// Default embeddinggemma-300m entry (mirrors first-boot defaults).
fn default_embed_entry() -> LlamacppEntry {
    LlamacppEntry {
        name: "embeddinggemma-300m".into(),
        model: ModelConfig {
            name: "embeddinggemma-300m".into(),
            sha256: "a0f7b4e13c397a6e1b32c2de75b1f65a14c92ec524d5f674d94a4290a1c4969b".into(),
            size_bytes: Some(328_577_056),
            source_url: "https://huggingface.co/unsloth/embeddinggemma-300m-GGUF/resolve/\
                 6661a6504c30d8304af13455cb4a5d4f5bc6011f/embeddinggemma-300M-Q8_0.gguf"
                .into(),
            license: Some("gemma".into()),
        },
        mmproj: None,
        mmproj_image_max_tokens: None,
        n_ctx: 2048,
        n_gpu_layers: -1,
        embed: true,
        embed_pooling: None,
        embed_n_ctx: 2048,
        // Bi-encoder: no classification head, so no rerank (ADR 0027).
        rerank: false,
        rerank_n_ctx: 2048,
        // Embedding model: no chat template, no renderer (ADR 0026).
        chat_template: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_file::ModelAutoselect;

    fn base_cfg(mode: ModelAutoselect) -> ConfigFile {
        let mut c = crate::config_file::default_first_boot_config();
        c.model_autoselect = mode;
        c.model_autoselect_min_vram_gib = 20;
        // Start from an empty backend list so we exercise synthesis.
        c.backends = Some(vec![]);
        c
    }

    // Same invariant `config_file::default_source_urls_pin_immutable_revisions`
    // enforces on the first-boot defaults, applied to the auto-select
    // tiers — which is where the 12B URLs live, and they drifted too.
    #[test]
    fn tier_default_source_urls_pin_immutable_revisions() {
        fn assert_pinned(url: &str, what: &str) {
            assert!(
                !url.contains("/resolve/main/"),
                "{what} points at the mutable `main` branch: {url}"
            );
            let rev = url
                .split("/resolve/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_else(|| panic!("{what} has no /resolve/<rev>/ segment: {url}"));
            assert!(
                rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit()),
                "{what} revision {rev:?} is not a 40-char commit hash: {url}"
            );
        }

        for tier in [Tier::E4b, Tier::B12] {
            let e = default_gen_entry(tier, 8192, -1);
            assert_pinned(&e.model.source_url, &format!("{tier:?} model"));
            let mm = e.mmproj.as_ref().expect("tier default carries an mmproj");
            assert_pinned(&mm.source_url, &format!("{tier:?} mmproj"));
        }
        assert_pinned(&default_embed_entry().model.source_url, "embed model");
    }

    #[test]
    fn tier_gates_on_total_not_free() {
        // 24 GB total → 12B, regardless of free.
        assert_eq!(pick_tier(Some(24 * GIB), 20), Tier::B12);
        // 16 GB total → E4B (below 20 GiB bar) even if fully free.
        assert_eq!(pick_tier(Some(16 * GIB), 20), Tier::E4b);
        // Exactly at the bar → 12B.
        assert_eq!(pick_tier(Some(20 * GIB), 20), Tier::B12);
        // Unknown memory (CPU / no report) → E4B (safe default).
        assert_eq!(pick_tier(None, 20), Tier::E4b);
    }

    #[test]
    fn autoselect_off_is_noop() {
        let mut c = base_cfg(ModelAutoselect::Off);
        assert!(apply(&mut c, Some(24 * GIB), Some(24 * GIB)).is_none());
    }

    #[test]
    fn explicit_gen_backend_overrides_autoselect() {
        let mut c = base_cfg(ModelAutoselect::Auto);
        // Pin an explicit generation backend.
        c.backends = Some(vec![BackendEntry::Llamacpp(default_gen_entry(
            Tier::E4b,
            8192,
            -1,
        ))]);
        assert!(
            apply(&mut c, Some(24 * GIB), Some(24 * GIB)).is_none(),
            "explicit gen backend must win"
        );
    }

    #[test]
    fn auto_24gb_picks_12b_and_synthesises_embed() {
        let mut c = base_cfg(ModelAutoselect::Auto);
        let out = apply(&mut c, Some(24 * GIB), Some(24 * GIB)).unwrap();
        assert_eq!(out.tier, Tier::B12);
        assert!(!out.embed_forced_cpu, "24 GB fits gen+embed on GPU");
        let backends = c.backends.unwrap();
        // gen (12b) + synthesised embed.
        assert_eq!(backends.len(), 2);
        match &backends[0] {
            BackendEntry::Llamacpp(l) => assert_eq!(l.name, "gemma-4-12b"),
            _ => panic!("gen not first"),
        }
        match &backends[1] {
            BackendEntry::Llamacpp(l) => {
                assert!(l.embed);
                assert_eq!(l.n_gpu_layers, -1, "embed stays on GPU with room");
            }
            _ => panic!("embed not second"),
        }
    }

    #[test]
    fn auto_16gb_picks_e4b() {
        let mut c = base_cfg(ModelAutoselect::Auto);
        let out = apply(&mut c, Some(16 * GIB), Some(13 * GIB)).unwrap();
        assert_eq!(out.tier, Tier::E4b);
    }

    #[test]
    fn embed_forced_cpu_when_free_too_tight() {
        // 12B tier (24 GB total) but only ~12 GB free right now: gen
        // (~11.8+1 GB) + embed + headroom won't fit → embed to CPU.
        let mut c = base_cfg(ModelAutoselect::Auto);
        let out = apply(&mut c, Some(24 * GIB), Some(12 * GIB)).unwrap();
        assert_eq!(out.tier, Tier::B12);
        assert!(
            out.embed_forced_cpu,
            "tight free VRAM should push embed to CPU"
        );
        let backends = c.backends.unwrap();
        match backends
            .iter()
            .find(|b| matches!(b, BackendEntry::Llamacpp(l) if l.embed))
        {
            Some(BackendEntry::Llamacpp(l)) => assert_eq!(l.n_gpu_layers, 0, "embed forced to CPU"),
            _ => panic!("no embed backend"),
        }
    }

    #[test]
    fn non_llamacpp_backends_preserved() {
        use crate::config_file::OpenaiCompatEntry;
        let mut c = base_cfg(ModelAutoselect::Auto);
        // An openai-compat fallback + the default embed.
        c.backends = Some(vec![BackendEntry::OpenaiCompat(OpenaiCompatEntry {
            name: "cloud-fallback".into(),
            base_url: "https://api.openai.com".into(),
            model: "gpt-4o-mini".into(),
            api_key_env: None,
            timeout_secs: 300,
        })]);
        let out = apply(&mut c, Some(24 * GIB), Some(24 * GIB)).unwrap();
        assert_eq!(out.tier, Tier::B12);
        let backends = c.backends.unwrap();
        assert!(
            backends.iter().any(|b| b.name() == "cloud-fallback"),
            "openai-compat backend must be preserved"
        );
        assert!(
            backends.iter().any(|b| b.name() == "gemma-4-12b"),
            "synthesised gen present"
        );
    }

    #[test]
    fn vram_estimate_12b_larger_than_e4b() {
        let e4b = estimate_gen_vram_bytes(Tier::E4b, 8192, true);
        let b12 = estimate_gen_vram_bytes(Tier::B12, 8192, true);
        assert!(b12 > e4b);
        // 12B@8k should land in the ~12-13 GB range (11.8 base + 1 mmproj).
        assert!((12 * GIB..=14 * GIB).contains(&b12), "12b@8k est = {b12}");
    }

    #[test]
    fn vram_estimate_grows_with_ctx() {
        let ctx8k = estimate_gen_vram_bytes(Tier::B12, 8192, false);
        let ctx32k = estimate_gen_vram_bytes(Tier::B12, 32768, false);
        assert!(ctx32k > ctx8k, "larger context needs more VRAM");
    }
}
