//! Operator-facing JSON config file.
//!
//! Default location: `~/.inferd/config.json` (Unix) /
//! `%USERPROFILE%\.inferd\config.json` (Windows). Override via
//! `--config` CLI flag or `INFERD_CONFIG` env var.
//!
//! # Schema (single-backend, legacy)
//!
//! ```json
//! {
//!   "auto_pull": true,
//!   "models_home": "~/.local/share/models",
//!   "model": {
//!     "name":       "gemma-4-e4b",
//!     "sha256":     "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
//!     "size_bytes": 5126304928,
//!     "source_url": "https://huggingface.co/unsloth/.../resolve/main/...gguf",
//!     "license":    "apache-2.0"
//!   },
//!   "n_ctx":         8192,
//!   "n_gpu_layers":  0,
//!   "admin_addr":    "/run/inferd/admin.sock"
//! }
//! ```
//!
//! # Schema (multi-backend, v0.2+)
//!
//! Per ADR 0007, the router walks an *ordered* list of backends; the
//! first that's `ready()` and not currently circuit-broken serves the
//! request. The config-file surface mirrors that:
//!
//! ```json
//! {
//!   "models_home": "~/.local/share/models",
//!   "backends": [
//!     {
//!       "kind": "llamacpp",
//!       "name": "local-gemma",
//!       "model": { "name": "gemma-4-e4b", "sha256": "...", "source_url": "https://...gguf" },
//!       "n_ctx": 8192,
//!       "n_gpu_layers": 35
//!     },
//!     {
//!       "kind": "openai-compat",
//!       "name": "anthropic-fallback",
//!       "base_url": "https://api.anthropic.com",
//!       "model": "claude-opus-4-7",
//!       "api_key_env": "ANTHROPIC_API_KEY",
//!       "timeout_secs": 300
//!     }
//!   ]
//! }
//! ```
//!
//! `backends:` and `model:` are mutually exclusive. When `model:` is
//! present (legacy single-backend shape), the daemon promotes it to a
//! one-element `backends:` list with `kind: "llamacpp"` so existing
//! v0.1.x configs keep working without edits.
//!
//! API keys for `openai-compat` are referenced by env-var **name**
//! via `api_key_env:` — never embedded literally in the file. The
//! daemon reads the named env at startup. When `api_key_env:` is
//! absent, falls back to `INFERD_OPENAI_API_KEY`, then `OPENAI_API_KEY`,
//! then empty (skips `Authorization` for self-hosted endpoints).
//!
//! The `kind:` field is an open-ended tagged union: future variants
//! (`bedrock-invoke`, `bedrock-converse`, etc.) slot in additively
//! without breaking existing configs.
//!
//! Resolution order for the model store (per ADR 0011):
//!
//! 1. `models_home` field if set in this config.
//! 2. `MODELS_HOME` env var.
//! 3. Platform default (XDG / Application Support / LOCALAPPDATA).
//!
//! CLI flags override config-file values when both are present.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

/// Top-level config-file schema.
///
/// Two flavours coexist:
///
/// - **Legacy single-backend** — `model:` at the top level, plus
///   `n_ctx` / `n_gpu_layers`. Implies one `kind: "llamacpp"` backend.
/// - **Multi-backend** — `backends: [...]` carries an ordered list of
///   backend entries. Router walks the list per ADR 0007.
///
/// The two are mutually exclusive at parse time: setting both is a
/// validation error. `auto_pull` and `admin_addr` apply to both
/// flavours.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    /// When `true` and a `kind: "llamacpp"` model file is absent, the
    /// daemon downloads it from the entry's `source_url` on startup.
    /// When `false`, the daemon refuses to start with a clear error
    /// pointing at the operator's next step. Default: `true`. Applies
    /// to every llamacpp entry in `backends:`.
    #[serde(default = "default_auto_pull")]
    pub auto_pull: bool,

    /// Override for the shared model store root. When unset the
    /// daemon falls back to `MODELS_HOME` env, then the platform
    /// default. Tilde-expanded on read.
    #[serde(default)]
    pub models_home: Option<PathBuf>,

    /// Legacy single-backend model spec. Deprecated in favour of
    /// `backends:` but kept for v0.1.x config-file compatibility.
    /// Mutually exclusive with `backends:`.
    #[serde(default)]
    pub model: Option<ModelConfig>,

    /// Llama.cpp context window in tokens. Default: 8192. Used as
    /// the fallback for legacy `model:` entries; multi-backend
    /// entries carry their own `n_ctx`.
    #[serde(default = "default_n_ctx")]
    pub n_ctx: u32,

    /// Llama.cpp GPU layer offload count. 0 = CPU-only. Default: 0.
    /// Used as the fallback for legacy `model:` entries; multi-
    /// backend entries carry their own `n_gpu_layers`.
    #[serde(default)]
    pub n_gpu_layers: i32,

    /// Admin socket address. Default: platform-specific path per
    /// `docs/protocol-v1.md` §"Admin endpoint".
    #[serde(default)]
    pub admin_addr: Option<String>,

    /// Ordered list of backends (multi-backend shape). First entry
    /// is highest priority — the router tries it first, then the
    /// next, etc. Mutually exclusive with `model:`.
    #[serde(default)]
    pub backends: Option<Vec<BackendEntry>>,

    /// Optional listener overrides. Default behaviour is unchanged:
    /// the operator picks a transport via `--tcp` / `--uds` /
    /// `--pipe` on the CLI. When `listen:` is present **and** the
    /// CLI did not pass a transport flag, the daemon binds the
    /// transports declared here. CLI flags always win when both
    /// are set. Restart-time only — no config watcher.
    #[serde(default)]
    pub listen: Option<ListenConfig>,
}

/// Operator-declared listener overrides. Every field is optional.
/// TCP is **off by default** — set `tcp:` (and `tcp_v2:` if running
/// with v2) to opt in for cross-VM use cases (WSL ↔ Windows host,
/// podman-on-machine, …) where Unix sockets / named pipes don't
/// cross the boundary cleanly. Mirrors the security shape of
/// `openai-compat`: the API key is referenced by env-var **name**,
/// never embedded literally in the file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListenConfig {
    /// Loopback TCP bind address for the v1 inference socket, e.g.
    /// `"127.0.0.1:9090"` or `"0.0.0.0:9090"`. When unset, no v1
    /// TCP listener is bound from config (CLI `--tcp` may still
    /// provide one). v0.1 invariant: CLI mutual exclusion still
    /// applies — if CLI passes `--uds` / `--pipe`, the config
    /// `tcp:` is ignored with a one-line warning at startup.
    #[serde(default)]
    pub tcp: Option<String>,

    /// Loopback TCP bind address for the v2 inference socket. Has
    /// no effect unless `--v2` is also set on the CLI.
    #[serde(default)]
    pub tcp_v2: Option<String>,

    /// **Name** of the env var carrying the pre-shared API key for
    /// TCP clients (THREAT_MODEL F-8). When set, the daemon reads
    /// the named env at startup and clients must send
    /// `{"type":"auth","key":"<value>"}` as their first NDJSON
    /// frame. UDS and named-pipe transports ignore this — kernel-
    /// attested peer credentials (F-7) gate those. CLI `--api-key`
    /// always wins when both are set.
    #[serde(default)]
    pub api_key_env: Option<String>,
}

/// A single backend declaration. Tagged on `kind:` so future
/// variants (`bedrock-invoke`, `bedrock-converse`, …) slot in
/// additively. Unknown kinds are rejected at parse time so operators
/// see a clear error rather than a silent skip.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum BackendEntry {
    /// Local llama.cpp backend over a GGUF file in the shared CAS.
    Llamacpp(LlamacppEntry),
    /// Outbound HTTPS adapter for any provider speaking the OpenAI
    /// Chat Completions wire (OpenAI, Anthropic via the compat layer
    /// at `api.anthropic.com/v1/`, OpenRouter, vLLM, LM Studio,
    /// LocalAI, Ollama, llama.cpp's HTTP server).
    OpenaiCompat(OpenaiCompatEntry),
}

impl BackendEntry {
    /// Operator-supplied stable identifier, used in router feedback
    /// and admin-status events.
    pub fn name(&self) -> &str {
        match self {
            BackendEntry::Llamacpp(e) => &e.name,
            BackendEntry::OpenaiCompat(e) => &e.name,
        }
    }
}

/// Llamacpp backend entry inside `backends:`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlamacppEntry {
    /// Stable operator-facing identifier, e.g. `"local-gemma"`. Used
    /// in router feedback + admin events. Required to be unique
    /// across all entries.
    pub name: String,

    /// Per-entry model spec (CAS layout, ADR 0011).
    pub model: ModelConfig,

    /// Llama.cpp context window in tokens. Default: 8192.
    #[serde(default = "default_n_ctx")]
    pub n_ctx: u32,

    /// Llama.cpp GPU layer offload count. 0 = CPU-only. Default: 0.
    #[serde(default)]
    pub n_gpu_layers: i32,
}

/// OpenAI-compat backend entry inside `backends:`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenaiCompatEntry {
    /// Stable operator-facing identifier, e.g. `"anthropic-fallback"`.
    pub name: String,

    /// Base URL of the upstream, no trailing slash and no path
    /// (the adapter appends `/v1/chat/completions`). Examples:
    /// `https://api.openai.com`, `https://api.anthropic.com`,
    /// `http://localhost:11434`.
    pub base_url: String,

    /// Upstream model identifier echoed in the request `model` field.
    /// Provider-specific (e.g. `gpt-4o-mini`, `claude-opus-4-7`,
    /// `llama3.1:8b`).
    pub model: String,

    /// **Name** of the env var carrying the bearer token — never the
    /// literal token. Operators set the env separately so secrets
    /// stay out of the config file. When unset, the daemon falls
    /// back to `INFERD_OPENAI_API_KEY`, then `OPENAI_API_KEY`, then
    /// skips the `Authorization` header (some self-hosted endpoints
    /// accept unauthenticated traffic).
    #[serde(default)]
    pub api_key_env: Option<String>,

    /// Total request timeout in seconds. Default 300 (5 minutes).
    #[serde(default = "default_openai_timeout_secs")]
    pub timeout_secs: u64,
}

/// Per-model entry: pinned URL + pinned SHA-256 + name.
///
/// The shape mirrors `fetch::ModelSpec` but as a serde-deserialisable
/// config-file type. Conversion is straightforward (`From` impl below).
///
/// Note: there is no `filename` field. The blob's on-disk location
/// is derived from its SHA-256 (CAS layout, ADR 0011); the manifest
/// at `<store>/manifests/<name>.json` is the only place a name maps
/// to a blob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Stable identifier, e.g. `"gemma-4-e4b"`. Used as the manifest
    /// filename and the lock-file basename.
    pub name: String,
    /// Lowercase hex SHA-256 of the GGUF bytes. Required.
    pub sha256: String,
    /// Advisory total size for progress reporting + manifest.
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Direct-download HTTPS endpoint. Must be `https://`.
    pub source_url: String,
    /// SPDX-style license id when known. Recorded in the manifest.
    #[serde(default)]
    pub license: Option<String>,
}

fn default_auto_pull() -> bool {
    true
}

fn default_n_ctx() -> u32 {
    8192
}

fn default_openai_timeout_secs() -> u64 {
    300
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
}

/// Default config-file path: `~/.inferd/config.json` on Unix /
/// `%USERPROFILE%\.inferd\config.json` on Windows. Honours
/// `INFERD_CONFIG` for tests and ops.
pub fn default_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("INFERD_CONFIG") {
        return PathBuf::from(p);
    }
    let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".inferd").join("config.json")
}

/// Errors produced by `ConfigFile::load`.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The config file did not exist at the resolved path.
    #[error("config file not found: {0}")]
    NotFound(PathBuf),
    /// I/O error reading the file.
    #[error("io reading {path}: {source}")]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// JSON parse failure.
    #[error("parse {path}: {source}")]
    Parse {
        /// Path that failed.
        path: PathBuf,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
    /// Validation failure on otherwise-well-formed config.
    #[error("invalid config: {0}")]
    Invalid(String),
}

impl ConfigFile {
    /// Read + parse + validate a config file at `path`.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let file = File::open(path).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                ConfigError::NotFound(path.to_path_buf())
            } else {
                ConfigError::Io {
                    path: path.to_path_buf(),
                    source: e,
                }
            }
        })?;
        let reader = BufReader::new(file);
        let mut cfg: ConfigFile =
            serde_json::from_reader(reader).map_err(|e| ConfigError::Parse {
                path: path.to_path_buf(),
                source: e,
            })?;
        cfg.expand_paths();
        cfg.validate()?;
        Ok(cfg)
    }

    fn expand_paths(&mut self) {
        if let Some(p) = self.models_home.as_ref()
            && let Some(stripped) = p
                .to_str()
                .and_then(|s| s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")))
            && let Some(home) = home_dir()
        {
            self.models_home = Some(home.join(stripped));
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        match (&self.model, &self.backends) {
            (Some(_), Some(_)) => {
                return Err(ConfigError::Invalid(
                    "config: `model` and `backends` are mutually exclusive — \
                     pick one shape, not both"
                        .into(),
                ));
            }
            (None, None) => {
                return Err(ConfigError::Invalid(
                    "config: must specify either `model` (legacy single-backend) \
                     or `backends` (multi-backend list)"
                        .into(),
                ));
            }
            _ => {}
        }
        if self.n_ctx == 0 {
            return Err(ConfigError::Invalid("n_ctx must be > 0".into()));
        }
        if let Some(m) = &self.model {
            validate_model_config(m)?;
        }
        if let Some(listen) = &self.listen {
            if let Some(addr) = &listen.tcp
                && addr.trim().is_empty()
            {
                return Err(ConfigError::Invalid(
                    "listen.tcp must not be empty when set".into(),
                ));
            }
            if let Some(addr) = &listen.tcp_v2
                && addr.trim().is_empty()
            {
                return Err(ConfigError::Invalid(
                    "listen.tcp_v2 must not be empty when set".into(),
                ));
            }
        }
        if let Some(list) = &self.backends {
            if list.is_empty() {
                return Err(ConfigError::Invalid(
                    "backends list must not be empty".into(),
                ));
            }
            let mut seen = std::collections::HashSet::with_capacity(list.len());
            for entry in list {
                let name = entry.name();
                if name.is_empty() {
                    return Err(ConfigError::Invalid(
                        "backends[].name must not be empty".into(),
                    ));
                }
                if !seen.insert(name.to_string()) {
                    return Err(ConfigError::Invalid(format!(
                        "duplicate backends[].name {name:?} — names must be unique"
                    )));
                }
                match entry {
                    BackendEntry::Llamacpp(e) => {
                        validate_model_config(&e.model)?;
                        if e.n_ctx == 0 {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].n_ctx must be > 0"
                            )));
                        }
                    }
                    BackendEntry::OpenaiCompat(e) => {
                        if e.base_url.trim().is_empty() {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].base_url must not be empty"
                            )));
                        }
                        if !(e.base_url.starts_with("https://")
                            || e.base_url.starts_with("http://"))
                        {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].base_url must be http:// or https:// \
                                 (got {:?})",
                                e.base_url
                            )));
                        }
                        if e.model.trim().is_empty() {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].model must not be empty"
                            )));
                        }
                        if e.timeout_secs == 0 {
                            return Err(ConfigError::Invalid(format!(
                                "backends[{name:?}].timeout_secs must be > 0"
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Canonical multi-backend list. When the operator wrote the
    /// legacy single-backend shape (`model:` at top level), this
    /// returns a one-element list with `kind: "llamacpp"` so the
    /// rest of the daemon only ever sees the multi-backend shape.
    pub fn resolved_backends(&self) -> Vec<BackendEntry> {
        if let Some(list) = &self.backends {
            return list.clone();
        }
        // Legacy promotion path. `validate()` ensures exactly one of
        // (`model`, `backends`) is set, so the unwrap is unreachable
        // for any value that reached this method.
        let m = self
            .model
            .as_ref()
            .expect("validate() guarantees one of model|backends is set")
            .clone();
        vec![BackendEntry::Llamacpp(LlamacppEntry {
            name: m.name.clone(),
            model: m,
            n_ctx: self.n_ctx,
            n_gpu_layers: self.n_gpu_layers,
        })]
    }
}

fn validate_model_config(m: &ModelConfig) -> Result<(), ConfigError> {
    if m.name.is_empty() {
        return Err(ConfigError::Invalid("model.name must not be empty".into()));
    }
    if !m.source_url.starts_with("https://") {
        return Err(ConfigError::Invalid(format!(
            "model.source_url must be https:// (got {:?})",
            m.source_url
        )));
    }
    if m.sha256.len() != 64
        || !m
            .sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(ConfigError::Invalid(
            "model.sha256 must be 64 lowercase hex chars".into(),
        ));
    }
    Ok(())
}

impl From<&ModelConfig> for crate::fetch::ModelSpec {
    fn from(m: &ModelConfig) -> Self {
        crate::fetch::ModelSpec {
            name: m.name.clone(),
            source_url: m.source_url.clone(),
            sha256_hex: m.sha256.clone(),
            size_bytes: m.size_bytes,
            license: m.license.clone(),
            source: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_config(s: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(s.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    fn good_json() -> String {
        r#"{
            "auto_pull": true,
            "models_home": "/tmp/inferd-models-home",
            "model": {
                "name": "gemma-4-e4b",
                "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                "size_bytes": 5126304928,
                "source_url": "https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF/resolve/main/gemma-4-E4B-it-UD-Q4_K_XL.gguf",
                "license": "apache-2.0"
            },
            "n_ctx": 8192,
            "n_gpu_layers": 0
        }"#
        .to_string()
    }

    #[test]
    fn load_well_formed_config() {
        let f = write_config(&good_json());
        let cfg = ConfigFile::load(f.path()).unwrap();
        let m = cfg.model.as_ref().expect("legacy model present");
        assert_eq!(m.name, "gemma-4-e4b");
        assert_eq!(m.size_bytes, Some(5_126_304_928));
        assert_eq!(m.license.as_deref(), Some("apache-2.0"));
        assert!(cfg.auto_pull);
        assert_eq!(cfg.n_ctx, 8192);
        assert_eq!(
            cfg.models_home,
            Some(PathBuf::from("/tmp/inferd-models-home"))
        );
    }

    #[test]
    fn missing_file_returns_not_found() {
        let path = std::env::temp_dir().join("inferd-config-does-not-exist.json");
        let _ = std::fs::remove_file(&path);
        let err = ConfigFile::load(&path).unwrap_err();
        assert!(matches!(err, ConfigError::NotFound(_)));
    }

    #[test]
    fn invalid_json_returns_parse_error() {
        let f = write_config("{ not valid json");
        let err = ConfigFile::load(f.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn http_url_rejected() {
        let bad = good_json().replace("https://", "http://");
        let f = write_config(&bad);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("https://")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn uppercase_sha_rejected() {
        let bad = good_json().replace(
            "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
            "30D1E7949597A3446726064E80B876FD1B5CBA4AA6EEC53D27AFA420E731FB36",
        );
        let f = write_config(&bad);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("lowercase hex")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn short_sha_rejected() {
        let bad = good_json().replace(
            "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
            "30d1e7",
        );
        let f = write_config(&bad);
        let err = ConfigFile::load(f.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn defaults_when_optional_fields_missing() {
        let json = r#"{
            "model": {
                "name": "gemma-4-e4b",
                "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                "source_url": "https://example.com/x.gguf"
            }
        }"#;
        let f = write_config(json);
        let cfg = ConfigFile::load(f.path()).unwrap();
        let m = cfg.model.as_ref().expect("legacy model present");
        assert!(cfg.auto_pull);
        assert_eq!(cfg.n_ctx, 8192);
        assert_eq!(cfg.n_gpu_layers, 0);
        assert!(m.size_bytes.is_none());
        assert!(cfg.models_home.is_none());
        assert!(m.license.is_none());
    }

    #[test]
    fn modelconfig_converts_to_fetch_modelspec() {
        let cfg = ModelConfig {
            name: "x".into(),
            sha256: "abc".into(),
            size_bytes: Some(42),
            source_url: "https://e/x.gguf".into(),
            license: Some("mit".into()),
        };
        let spec: crate::fetch::ModelSpec = (&cfg).into();
        assert_eq!(spec.name, "x");
        assert_eq!(spec.size_bytes, Some(42));
        assert_eq!(spec.sha256_hex, "abc");
        assert_eq!(spec.license.as_deref(), Some("mit"));
    }

    fn good_multi_backend_json() -> String {
        r#"{
            "models_home": "/tmp/inferd-models-home",
            "backends": [
                {
                    "kind": "llamacpp",
                    "name": "local-gemma",
                    "model": {
                        "name": "gemma-4-e4b",
                        "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                        "source_url": "https://example.com/gemma.gguf"
                    },
                    "n_ctx": 8192,
                    "n_gpu_layers": 35
                },
                {
                    "kind": "openai-compat",
                    "name": "anthropic-fallback",
                    "base_url": "https://api.anthropic.com",
                    "model": "claude-opus-4-7",
                    "api_key_env": "ANTHROPIC_API_KEY"
                }
            ]
        }"#
        .to_string()
    }

    #[test]
    fn load_multi_backend_config() {
        let f = write_config(&good_multi_backend_json());
        let cfg = ConfigFile::load(f.path()).unwrap();
        assert!(cfg.model.is_none());
        let list = cfg.backends.as_ref().expect("backends present");
        assert_eq!(list.len(), 2);
        match &list[0] {
            BackendEntry::Llamacpp(e) => {
                assert_eq!(e.name, "local-gemma");
                assert_eq!(e.model.name, "gemma-4-e4b");
                assert_eq!(e.n_ctx, 8192);
                assert_eq!(e.n_gpu_layers, 35);
            }
            other => panic!("expected llamacpp, got {other:?}"),
        }
        match &list[1] {
            BackendEntry::OpenaiCompat(e) => {
                assert_eq!(e.name, "anthropic-fallback");
                assert_eq!(e.base_url, "https://api.anthropic.com");
                assert_eq!(e.model, "claude-opus-4-7");
                assert_eq!(e.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
                assert_eq!(e.timeout_secs, 300);
            }
            other => panic!("expected openai-compat, got {other:?}"),
        }
    }

    #[test]
    fn rejects_both_model_and_backends() {
        let json = r#"{
            "model": {
                "name": "gemma-4-e4b",
                "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                "source_url": "https://example.com/x.gguf"
            },
            "backends": [
                {
                    "kind": "openai-compat",
                    "name": "x",
                    "base_url": "https://api.openai.com",
                    "model": "gpt-4o-mini"
                }
            ]
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("mutually exclusive")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_neither_model_nor_backends() {
        let json = r#"{ "auto_pull": true }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("must specify either")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_backends_list() {
        let json = r#"{ "backends": [] }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("must not be empty")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_duplicate_backend_names() {
        let json = r#"{
            "backends": [
                {
                    "kind": "openai-compat",
                    "name": "dup",
                    "base_url": "https://api.openai.com",
                    "model": "gpt-4o-mini"
                },
                {
                    "kind": "openai-compat",
                    "name": "dup",
                    "base_url": "https://api.anthropic.com",
                    "model": "claude-opus-4-7"
                }
            ]
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("duplicate")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_openai_compat_without_base_url() {
        let json = r#"{
            "backends": [
                {
                    "kind": "openai-compat",
                    "name": "x",
                    "base_url": "",
                    "model": "gpt-4o-mini"
                }
            ]
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn rejects_openai_compat_with_bad_scheme() {
        let json = r#"{
            "backends": [
                {
                    "kind": "openai-compat",
                    "name": "x",
                    "base_url": "ftp://api.openai.com",
                    "model": "gpt-4o-mini"
                }
            ]
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("http")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn accepts_openai_compat_with_localhost_http() {
        let json = r#"{
            "backends": [
                {
                    "kind": "openai-compat",
                    "name": "ollama",
                    "base_url": "http://localhost:11434",
                    "model": "llama3.1:8b"
                }
            ]
        }"#;
        let f = write_config(json);
        let cfg = ConfigFile::load(f.path()).unwrap();
        assert_eq!(cfg.resolved_backends().len(), 1);
    }

    #[test]
    fn rejects_unknown_kind() {
        let json = r#"{
            "backends": [
                {
                    "kind": "future-bedrock-thing",
                    "name": "x"
                }
            ]
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn legacy_model_promotes_to_one_backend() {
        let f = write_config(&good_json());
        let cfg = ConfigFile::load(f.path()).unwrap();
        let resolved = cfg.resolved_backends();
        assert_eq!(resolved.len(), 1);
        match &resolved[0] {
            BackendEntry::Llamacpp(e) => {
                assert_eq!(e.name, "gemma-4-e4b");
                assert_eq!(e.n_ctx, 8192);
                assert_eq!(e.n_gpu_layers, 0);
            }
            other => panic!("expected llamacpp, got {other:?}"),
        }
    }

    #[test]
    fn multi_backend_resolved_passes_through() {
        let f = write_config(&good_multi_backend_json());
        let cfg = ConfigFile::load(f.path()).unwrap();
        let resolved = cfg.resolved_backends();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].name(), "local-gemma");
        assert_eq!(resolved[1].name(), "anthropic-fallback");
    }

    #[test]
    fn listen_block_absent_by_default() {
        let f = write_config(&good_json());
        let cfg = ConfigFile::load(f.path()).unwrap();
        assert!(cfg.listen.is_none());
    }

    #[test]
    fn listen_block_carries_tcp_and_api_key_env() {
        let json = r#"{
            "model": {
                "name": "gemma-4-e4b",
                "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                "source_url": "https://example.com/x.gguf"
            },
            "listen": {
                "tcp": "127.0.0.1:9090",
                "tcp_v2": "127.0.0.1:9091",
                "api_key_env": "INFERD_TCP_API_KEY"
            }
        }"#;
        let f = write_config(json);
        let cfg = ConfigFile::load(f.path()).unwrap();
        let listen = cfg.listen.as_ref().expect("listen present");
        assert_eq!(listen.tcp.as_deref(), Some("127.0.0.1:9090"));
        assert_eq!(listen.tcp_v2.as_deref(), Some("127.0.0.1:9091"));
        assert_eq!(listen.api_key_env.as_deref(), Some("INFERD_TCP_API_KEY"));
    }

    #[test]
    fn listen_rejects_empty_tcp() {
        let json = r#"{
            "model": {
                "name": "gemma-4-e4b",
                "sha256": "30d1e7949597a3446726064e80b876fd1b5cba4aa6eec53d27afa420e731fb36",
                "source_url": "https://example.com/x.gguf"
            },
            "listen": { "tcp": "   " }
        }"#;
        let f = write_config(json);
        let err = ConfigFile::load(f.path()).unwrap_err();
        match err {
            ConfigError::Invalid(msg) => assert!(msg.contains("listen.tcp")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }
}
