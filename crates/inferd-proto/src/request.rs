//! Request envelope and validation.

use crate::error::ProtoError;
use serde::{Deserialize, Serialize};

/// Gemma 4 sampling defaults applied when a Request omits the field.
///
/// Mirrors `docs/protocol-v1.md` §Request.
pub mod defaults {
    /// Default temperature when omitted.
    pub const TEMPERATURE: f64 = 1.0;
    /// Default top-p when omitted.
    pub const TOP_P: f64 = 0.95;
    /// Default top-k when omitted.
    pub const TOP_K: u32 = 64;
    /// Default max-tokens when omitted.
    pub const MAX_TOKENS: u32 = 1000;
    /// Default streaming behaviour when omitted.
    pub const STREAM: bool = true;
}

/// The set of `image_token_budget` values accepted by the daemon. Any other
/// value is rejected with `ErrorCode::InvalidRequest`.
pub const VALID_IMAGE_TOKEN_BUDGETS: [u32; 5] = [70, 140, 280, 560, 1120];

/// Conversation role attached to each message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System prompt setting overall instructions.
    System,
    /// End-user input.
    User,
    /// Prior model output replayed for context.
    Assistant,
}

/// One conversation turn carried in `Request::messages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Speaker.
    pub role: Role,
    /// Plain UTF-8 content for this turn. v1 does not support multimodal
    /// content arrays; image inputs are signalled via `Request::image_token_budget`.
    pub content: String,
}

/// Image-token budget; one of `VALID_IMAGE_TOKEN_BUDGETS`. Wraps a `u32` so
/// constructors can enforce the enum at the type level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageTokenBudget(u32);

impl ImageTokenBudget {
    /// Construct an `ImageTokenBudget` if the value is one of the accepted constants.
    pub fn new(value: u32) -> Option<Self> {
        if VALID_IMAGE_TOKEN_BUDGETS.contains(&value) {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Inner numeric value.
    pub fn get(self) -> u32 {
        self.0
    }
}

/// The inference request envelope sent by clients.
///
/// `Default` produces an empty request: empty `messages`, all
/// sampling fields `None` (server applies Gemma 4 defaults), no
/// grammar. Useful for the `..Default::default()` shorthand in
/// callers; remember to fill in `messages` before sending.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Request {
    /// Caller-assigned correlation id; echoed on every response frame.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,

    /// Conversation history in chronological order. Must be non-empty.
    pub messages: Vec<Message>,

    /// Sampling temperature; default `defaults::TEMPERATURE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// Nucleus sampling probability; default `defaults::TOP_P`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,

    /// Top-k sampling cutoff; default `defaults::TOP_K`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,

    /// Maximum tokens to generate; default `defaults::MAX_TOKENS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Stream tokens vs return one final `done` frame; default `defaults::STREAM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,

    /// Image token budget; if present, must be in `VALID_IMAGE_TOKEN_BUDGETS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_token_budget: Option<u32>,

    /// GBNF grammar to constrain generation; empty means unconstrained.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub grammar: String,
}

/// `Request` with all defaults applied and validation completed. Backends
/// receive this; they never see the optional-shaped wire form.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// Caller-assigned correlation id; echoed on every response frame.
    pub id: String,
    /// Conversation history.
    pub messages: Vec<Message>,
    /// Effective sampling temperature.
    pub temperature: f64,
    /// Effective nucleus sampling probability.
    pub top_p: f64,
    /// Effective top-k cutoff.
    pub top_k: u32,
    /// Effective max-tokens cap.
    pub max_tokens: u32,
    /// Effective streaming flag.
    pub stream: bool,
    /// Image token budget if the request declared one; `None` for text-only.
    pub image_token_budget: Option<ImageTokenBudget>,
    /// GBNF grammar; empty means unconstrained.
    pub grammar: String,
}

impl Request {
    /// Validate the request and apply Gemma 4 defaults to omitted fields.
    pub fn resolve(self) -> Result<Resolved, ProtoError> {
        if self.messages.is_empty() {
            return Err(ProtoError::InvalidRequest(
                "messages must not be empty".into(),
            ));
        }

        let image_token_budget = match self.image_token_budget {
            Some(v) => Some(ImageTokenBudget::new(v).ok_or_else(|| {
                ProtoError::InvalidRequest(format!(
                    "image_token_budget {v} not in {VALID_IMAGE_TOKEN_BUDGETS:?}"
                ))
            })?),
            None => None,
        };

        Ok(Resolved {
            id: self.id,
            messages: self.messages,
            temperature: self.temperature.unwrap_or(defaults::TEMPERATURE),
            top_p: self.top_p.unwrap_or(defaults::TOP_P),
            top_k: self.top_k.unwrap_or(defaults::TOP_K),
            max_tokens: self.max_tokens.unwrap_or(defaults::MAX_TOKENS),
            stream: self.stream.unwrap_or(defaults::STREAM),
            image_token_budget,
            grammar: self.grammar,
        })
    }
}
