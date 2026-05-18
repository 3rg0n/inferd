//! Operator-facing JSON config file.
//!
//! Default location: `~/.inferd/config.json` (Unix) /
//! `%USERPROFILE%\.inferd\config.json` (Windows). Override via
//! `--config` CLI flag or `INFERD_CONFIG` env var.
//!
//! Schema:
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
//! Resolution order for the model store (per ADR 0011):
//!
//! 1. `models_home` field if set in this config.
//! 2. `MODELS_HOME` env var.
//! 3. Platform default (XDG / Application Support / LOCALAPPDATA).
//!
//! All fields except `model` are optional. CLI flags override
//! config-file values when both are present.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

/// Top-level config-file schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    /// When `true` and the model file is absent, the daemon downloads
    /// it from `model.source_url` on startup. When `false`, the daemon
    /// refuses to start with a clear error pointing at the operator's
    /// next step. Default: `true`.
    #[serde(default = "default_auto_pull")]
    pub auto_pull: bool,

    /// Override for the shared model store root. When unset the
    /// daemon falls back to `MODELS_HOME` env, then the platform
    /// default. Tilde-expanded on read.
    #[serde(default)]
    pub models_home: Option<PathBuf>,

    /// The model the daemon serves on this run. Required.
    pub model: ModelConfig,

    /// Llama.cpp context window in tokens. Default: 8192.
    #[serde(default = "default_n_ctx")]
    pub n_ctx: u32,

    /// Llama.cpp GPU layer offload count. 0 = CPU-only. Default: 0.
    #[serde(default)]
    pub n_gpu_layers: i32,

    /// Admin socket address. Default: platform-specific path per
    /// `docs/protocol-v1.md` §"Admin endpoint".
    #[serde(default)]
    pub admin_addr: Option<String>,
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
        if let Some(p) = self.models_home.as_ref() {
            if let Some(stripped) = p
                .to_str()
                .and_then(|s| s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")))
            {
                if let Some(home) = home_dir() {
                    self.models_home = Some(home.join(stripped));
                }
            }
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.model.name.is_empty() {
            return Err(ConfigError::Invalid("model.name must not be empty".into()));
        }
        if !self.model.source_url.starts_with("https://") {
            return Err(ConfigError::Invalid(format!(
                "model.source_url must be https:// (got {:?})",
                self.model.source_url
            )));
        }
        if self.model.sha256.len() != 64
            || !self
                .model
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(ConfigError::Invalid(
                "model.sha256 must be 64 lowercase hex chars".into(),
            ));
        }
        if self.n_ctx == 0 {
            return Err(ConfigError::Invalid("n_ctx must be > 0".into()));
        }
        Ok(())
    }
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
        assert_eq!(cfg.model.name, "gemma-4-e4b");
        assert_eq!(cfg.model.size_bytes, Some(5_126_304_928));
        assert_eq!(cfg.model.license.as_deref(), Some("apache-2.0"));
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
        assert!(cfg.auto_pull);
        assert_eq!(cfg.n_ctx, 8192);
        assert_eq!(cfg.n_gpu_layers, 0);
        assert!(cfg.model.size_bytes.is_none());
        assert!(cfg.models_home.is_none());
        assert!(cfg.model.license.is_none());
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
}
