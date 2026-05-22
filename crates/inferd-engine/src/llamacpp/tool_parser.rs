//! Streaming parser for Gemma 4 tool-call sequences.
//!
//! Gemma 4 emits tool calls inline in its token stream as:
//! ```text
//! <|tool_call>call:NAME{KEY:<|"|>VALUE<|"|>,...}<tool_call|>
//! ```
//! (See `docs/text.function.calling.with.gemma.4.md`.)
//!
//! This parser feeds the adapter's per-token text pieces in and
//! yields one of:
//!   - `Output::Text(s)` — pass-through user-visible text.
//!   - `Output::ToolUse { ... }` — a complete parsed tool-call.
//!   - `Output::Malformed(reason)` — model emitted an opener but the
//!     body didn't parse cleanly.
//!
//! The parser is intentionally small and deterministic. It does NOT
//! depend on llama.cpp; it operates on string fragments only. That
//! makes it unit-testable against synthetic streams without booting
//! a real model.
//!
//! ## Token-boundary handling
//!
//! Gemma's tokenizer often splits the opener / closer sentinels
//! across multiple piece boundaries (e.g. `<|tool_` then `call>` ).
//! The parser maintains a small buffer of "pending" bytes that hasn't
//! yet been classified as definitely-text or definitely-fence. When
//! the pending buffer holds a strict prefix of either sentinel, we
//! hold the bytes until a future piece either completes the match
//! (transition state) or invalidates it (flush as text).

use inferd_proto::v2::{ToolCallId, ToolUseInput};
use serde_json::Value;

/// One unit of output produced by [`ToolCallParser::push`].
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    /// Pass-through user-visible text. Stream straight to the wire.
    Text(String),
    /// Reasoning-trace text emitted between `<|think|>...<|/think|>`
    /// (per `docs/thinking.mode.in.gemma.md`). The wire emits this
    /// as `ResponseBlock::Thinking { delta }`.
    Thinking(String),
    /// A complete parsed tool-call sequence.
    ToolUse {
        /// Stable id paired with the consumer's eventual ToolResult.
        tool_call_id: ToolCallId,
        /// Tool name extracted from the `call:NAME{...}` payload.
        name: String,
        /// JSON arguments parsed from the payload body. Always an
        /// object even when the parser had to do shape coercion.
        input: ToolUseInput,
    },
    /// Model emitted a tool-call opener but the body could not be
    /// parsed cleanly. The adapter should terminate the stream with
    /// `ErrorCodeV2::ToolCallMalformed`.
    Malformed(String),
}

const TOOL_OPEN: &str = "<|tool_call>";
const TOOL_CLOSE: &str = "<tool_call|>";
const THINK_OPEN: &str = "<|think|>";
const THINK_CLOSE: &str = "<|/think|>";

/// State of the parser.
#[derive(Debug, Clone, PartialEq)]
enum State {
    /// Default: looking for either a tool-call opener or a thinking
    /// opener; everything else flushes as text.
    Plain,
    /// Inside a tool-call body. Accumulating until we see the closer.
    InToolCall,
    /// Inside a thinking block. Accumulating until we see the closer.
    InThinking,
}

/// Streaming parser. One per generation.
#[derive(Debug, Clone)]
pub struct ToolCallParser {
    state: State,
    /// Pending bytes that may be a prefix of a sentinel. Cleared when
    /// the prefix is invalidated or when a sentinel matches.
    pending: String,
    /// Bytes accumulated inside a tool-call or thinking body.
    body: String,
    /// Monotonic counter so ToolCallId stays unique per generation.
    n_calls: u32,
}

impl Default for ToolCallParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolCallParser {
    /// Construct a parser in the default `Plain` state.
    pub fn new() -> Self {
        Self {
            state: State::Plain,
            pending: String::with_capacity(32),
            body: String::with_capacity(256),
            n_calls: 0,
        }
    }

    /// Feed one token piece. Returns zero or more `Output`s.
    pub fn push(&mut self, piece: &str) -> Vec<Output> {
        let mut out = Vec::new();
        // Append the piece to the pending buffer first; we re-scan
        // pending each time so sentinels split across token
        // boundaries get caught.
        self.pending.push_str(piece);
        self.process(&mut out);
        out
    }

    /// Flush at end-of-stream. Any remaining pending bytes become
    /// trailing text. If we're still inside a tool-call or thinking
    /// body, emit a Malformed result — incomplete fences are a
    /// model-side bug.
    pub fn finish(mut self) -> Vec<Output> {
        let mut out = Vec::new();
        match self.state {
            State::Plain => {
                if !self.pending.is_empty() {
                    out.push(Output::Text(std::mem::take(&mut self.pending)));
                }
            }
            State::InToolCall => {
                out.push(Output::Malformed(
                    "stream ended inside <|tool_call>...<tool_call|> sequence".into(),
                ));
            }
            State::InThinking => {
                // Unclosed thinking blocks are recoverable — emit
                // what we got as Thinking content.
                if !self.body.is_empty() {
                    out.push(Output::Thinking(std::mem::take(&mut self.body)));
                }
            }
        }
        out
    }

    fn process(&mut self, out: &mut Vec<Output>) {
        loop {
            match self.state {
                State::Plain => {
                    if !self.advance_plain(out) {
                        return;
                    }
                }
                State::InToolCall => {
                    if !self.advance_tool(out) {
                        return;
                    }
                }
                State::InThinking => {
                    if !self.advance_thinking(out) {
                        return;
                    }
                }
            }
        }
    }

    /// Plain-state scanning: look for the earliest occurrence of any
    /// fence opener inside `pending`. Bytes before the opener flush
    /// as text. If `pending` ends with a strict prefix of any
    /// opener, we hold those bytes (they might complete on the next
    /// push). Returns `true` if a state transition happened (caller
    /// re-loops); `false` if we're done with this push.
    fn advance_plain(&mut self, out: &mut Vec<Output>) -> bool {
        // Look for the earliest opener.
        let tool_idx = self.pending.find(TOOL_OPEN);
        let think_idx = self.pending.find(THINK_OPEN);

        let (idx, opener_len, next_state) = match (tool_idx, think_idx) {
            (Some(i), Some(j)) if i <= j => (i, TOOL_OPEN.len(), State::InToolCall),
            (Some(i), None) => (i, TOOL_OPEN.len(), State::InToolCall),
            (None, Some(j)) => (j, THINK_OPEN.len(), State::InThinking),
            (Some(_), Some(j)) => (j, THINK_OPEN.len(), State::InThinking),
            (None, None) => {
                // No opener. Flush everything-except-trailing-prefix
                // as Text and stop.
                let safe_to_emit = self.safe_plain_emit_len();
                if safe_to_emit > 0 {
                    let t: String = self.pending.drain(..safe_to_emit).collect();
                    out.push(Output::Text(t));
                }
                return false;
            }
        };

        // Emit the prefix-before-opener as text.
        if idx > 0 {
            let t: String = self.pending.drain(..idx).collect();
            out.push(Output::Text(t));
        }
        // Drop the opener itself from pending and transition.
        self.pending.drain(..opener_len);
        self.state = next_state;
        true
    }

    /// Compute how many leading bytes of `pending` we can safely
    /// emit as text without losing a partial sentinel match. That's
    /// `pending.len() - longest_prefix_of_any_sentinel(pending)`.
    fn safe_plain_emit_len(&self) -> usize {
        let n = self.pending.len();
        // For each suffix length `k` (1..=min(n, max_opener_len)),
        // check whether the suffix is a strict prefix of any opener.
        // If yes, hold the last `k` bytes back. Take the largest
        // matching k.
        let max_len = TOOL_OPEN.len().max(THINK_OPEN.len());
        let mut hold = 0;
        for k in 1..=n.min(max_len) {
            let suffix = &self.pending[n - k..];
            if TOOL_OPEN.starts_with(suffix) || THINK_OPEN.starts_with(suffix) {
                hold = k;
            }
        }
        n - hold
    }

    /// Inside-tool scanning: accumulate `pending` into `body` until
    /// we see the closer. When found, parse the body and emit
    /// `Output::ToolUse` (or `Malformed`).
    fn advance_tool(&mut self, out: &mut Vec<Output>) -> bool {
        if let Some(idx) = self.pending.find(TOOL_CLOSE) {
            // Move bytes-up-to-closer into body, drop the closer.
            self.body.push_str(&self.pending[..idx]);
            self.pending.drain(..idx + TOOL_CLOSE.len());

            // Parse.
            let body = std::mem::take(&mut self.body);
            self.state = State::Plain;
            self.n_calls = self.n_calls.saturating_add(1);
            let id = ToolCallId::from(format!("tc-{}", self.n_calls));
            match parse_tool_call_body(&body) {
                Ok((name, input)) => out.push(Output::ToolUse {
                    tool_call_id: id,
                    name,
                    input,
                }),
                Err(reason) => out.push(Output::Malformed(reason)),
            }
            true
        } else {
            // No closer yet. Move all bytes-except-trailing-prefix
            // into body. Hold the trailing prefix in `pending`
            // because it might be the start of <tool_call|>.
            let n = self.pending.len();
            let max_len = TOOL_CLOSE.len();
            let mut hold = 0;
            for k in 1..=n.min(max_len) {
                let suffix = &self.pending[n - k..];
                if TOOL_CLOSE.starts_with(suffix) {
                    hold = k;
                }
            }
            let take = n - hold;
            if take > 0 {
                let chunk: String = self.pending.drain(..take).collect();
                self.body.push_str(&chunk);
            }
            false
        }
    }

    /// Inside-thinking scanning: same shape as advance_tool, but
    /// the body is emitted as `Output::Thinking` and isn't parsed.
    fn advance_thinking(&mut self, out: &mut Vec<Output>) -> bool {
        if let Some(idx) = self.pending.find(THINK_CLOSE) {
            self.body.push_str(&self.pending[..idx]);
            self.pending.drain(..idx + THINK_CLOSE.len());
            let body = std::mem::take(&mut self.body);
            self.state = State::Plain;
            if !body.is_empty() {
                out.push(Output::Thinking(body));
            }
            true
        } else {
            let n = self.pending.len();
            let max_len = THINK_CLOSE.len();
            let mut hold = 0;
            for k in 1..=n.min(max_len) {
                let suffix = &self.pending[n - k..];
                if THINK_CLOSE.starts_with(suffix) {
                    hold = k;
                }
            }
            let take = n - hold;
            if take > 0 {
                let chunk: String = self.pending.drain(..take).collect();
                self.body.push_str(&chunk);
            }
            false
        }
    }
}

/// Parse `call:NAME{KEY:<|"|>VALUE<|"|>,...}` into `(name, input)`.
/// The body uses Gemma's wire format (bare keys + `<|"|>`-quoted
/// strings). This is the inverse of the chat-template renderer's
/// inline-args path. Best-effort — if parse fails we return Err.
fn parse_tool_call_body(body: &str) -> Result<(String, ToolUseInput), String> {
    // Strip leading `call:` (per docs line 89).
    let body = body
        .trim()
        .strip_prefix("call:")
        .ok_or_else(|| format!("missing 'call:' prefix in tool-call body: {body:?}"))?;

    // Find the opening `{`.
    let lbrace = body
        .find('{')
        .ok_or_else(|| format!("missing '{{' in tool-call body: {body:?}"))?;
    let name = body[..lbrace].trim().to_string();
    if name.is_empty() {
        return Err("empty tool name in tool-call body".into());
    }

    // Strip the matching `}` at the end.
    let payload = body[lbrace..]
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(|| format!("unbalanced '{{' / '}}' in tool-call body: {body:?}"))?;

    // Convert Gemma's `<|"|>` string fences to JSON `"` quoting so
    // serde_json can parse the resulting object.
    let json_like = format!("{{{}}}", payload.replace("<|\"|>", "\""));
    // Wrap bare keys in double quotes. This is a small targeted
    // transform: keys are alphanumeric / underscore identifiers
    // followed by `:`. We walk the string and quote every such
    // identifier when it's at the start or after `,`.
    let json_like = quote_bare_keys(&json_like);

    let value: Value = serde_json::from_str(&json_like)
        .map_err(|e| format!("tool-call payload not valid JSON ({e}): {json_like:?}"))?;
    if !value.is_object() {
        return Err(format!(
            "tool-call payload must be a JSON object, got: {value:?}"
        ));
    }
    Ok((name, value))
}

/// Wrap bare keys in JSON-style quotes. Walks the string once;
/// inserts quotes around `IDENT:` where IDENT is an alphanumeric /
/// underscore identifier appearing at the start of input, after a
/// `,`, or after a `{`. Skips anything inside an existing string
/// literal.
fn quote_bare_keys(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            out.push(b as char);
            if b == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if b == b'"' {
            in_string = true;
            out.push('"');
            i += 1;
            continue;
        }
        let at_key_start = (i == 0 && b.is_ascii_alphabetic()) || {
            // Look back for `,` or `{` (skipping whitespace).
            let mut j = i;
            while j > 0 && (bytes[j - 1] == b' ' || bytes[j - 1] == b'\t') {
                j -= 1;
            }
            j > 0 && (bytes[j - 1] == b',' || bytes[j - 1] == b'{') && b.is_ascii_alphabetic()
        };
        if at_key_start {
            // Read the identifier.
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &s[start..i];
            // Only quote if followed by `:` (allowing whitespace).
            let mut k = i;
            while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                k += 1;
            }
            if k < bytes.len() && bytes[k] == b':' {
                out.push('"');
                out.push_str(ident);
                out.push('"');
            } else {
                out.push_str(ident);
            }
            continue;
        }
        out.push(b as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(parser: &mut ToolCallParser, pieces: &[&str]) -> Vec<Output> {
        let mut all = Vec::new();
        for p in pieces {
            all.extend(parser.push(p));
        }
        all
    }

    #[test]
    fn plain_text_passes_through() {
        let mut p = ToolCallParser::new();
        let out = collect(&mut p, &["hello ", "world"]);
        // The trailing piece doesn't end in a sentinel-prefix so it
        // should flush. With the prefix-hold logic, the last bytes
        // could be held — but neither "h", "he", ... nor "w", "wo",
        // ... is a prefix of `<|tool_call>` or `<|think|>`, so
        // everything flushes.
        let final_out = p.finish();
        let mut joined = String::new();
        for o in out.iter().chain(final_out.iter()) {
            if let Output::Text(t) = o {
                joined.push_str(t);
            }
        }
        assert_eq!(joined, "hello world");
    }

    #[test]
    fn complete_tool_call_in_one_piece() {
        let mut p = ToolCallParser::new();
        let out = p.push("<|tool_call>call:get_weather{location:<|\"|>London<|\"|>}<tool_call|>");
        assert_eq!(out.len(), 1);
        match &out[0] {
            Output::ToolUse {
                name,
                input,
                tool_call_id,
            } => {
                assert_eq!(name, "get_weather");
                assert_eq!(input["location"], "London");
                assert_eq!(tool_call_id.as_str(), "tc-1");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_split_across_pieces() {
        let mut p = ToolCallParser::new();
        let out = collect(
            &mut p,
            &[
                "<|tool_",
                "call>call:",
                "ping{",
                "host:<|\"|>",
                "example.com",
                "<|\"|>}",
                "<tool_call|>",
            ],
        );
        let tool_uses: Vec<_> = out
            .iter()
            .filter(|o| matches!(o, Output::ToolUse { .. }))
            .collect();
        assert_eq!(tool_uses.len(), 1, "got: {out:#?}");
        if let Output::ToolUse { name, input, .. } = tool_uses[0] {
            assert_eq!(name, "ping");
            assert_eq!(input["host"], "example.com");
        }
    }

    #[test]
    fn text_then_tool_call_then_text() {
        let mut p = ToolCallParser::new();
        let out = collect(
            &mut p,
            &[
                "I'll check.",
                "<|tool_call>call:lookup{q:<|\"|>x<|\"|>}<tool_call|>",
                "Here's the answer.",
            ],
        );
        // Expect Text("I'll check.") + ToolUse + Text("Here's the answer.")
        // (the last text might be held as a prefix; finish() flushes.)
        let final_out = p.finish();
        let mut texts = Vec::new();
        let mut tool_uses = 0;
        for o in out.iter().chain(final_out.iter()) {
            match o {
                Output::Text(t) => texts.push(t.clone()),
                Output::ToolUse { .. } => tool_uses += 1,
                _ => {}
            }
        }
        assert_eq!(tool_uses, 1);
        let joined = texts.join("");
        assert!(joined.contains("I'll check."));
        assert!(joined.contains("Here's the answer."));
    }

    #[test]
    fn malformed_tool_call_returns_malformed() {
        let mut p = ToolCallParser::new();
        let out = p.push("<|tool_call>not actually a function call<tool_call|>");
        assert!(matches!(out[0], Output::Malformed(_)), "got: {out:?}");
    }

    #[test]
    fn unclosed_tool_call_at_finish_is_malformed() {
        let mut p = ToolCallParser::new();
        let _ = p.push("<|tool_call>call:foo{x:<|\"|>y");
        let final_out = p.finish();
        assert!(matches!(final_out[0], Output::Malformed(_)));
    }

    #[test]
    fn thinking_block_emits_thinking() {
        let mut p = ToolCallParser::new();
        let out = collect(
            &mut p,
            &[
                "Let me ",
                "<|think|>this is hidden<|/think|>",
                " here's the result.",
            ],
        );
        let final_out = p.finish();
        let mut thinking = Vec::new();
        let mut texts = Vec::new();
        for o in out.iter().chain(final_out.iter()) {
            match o {
                Output::Thinking(t) => thinking.push(t.clone()),
                Output::Text(t) => texts.push(t.clone()),
                _ => {}
            }
        }
        assert_eq!(thinking.join(""), "this is hidden");
        let joined = texts.join("");
        assert!(joined.contains("Let me"));
        assert!(joined.contains("here's the result."));
    }

    #[test]
    fn thinking_split_across_pieces() {
        let mut p = ToolCallParser::new();
        let out = collect(
            &mut p,
            &["<|", "think|>", "secret", "<|/", "think|>", "visible"],
        );
        let final_out = p.finish();
        let mut thinking = String::new();
        let mut texts = String::new();
        for o in out.iter().chain(final_out.iter()) {
            match o {
                Output::Thinking(t) => thinking.push_str(t),
                Output::Text(t) => texts.push_str(t),
                _ => {}
            }
        }
        assert_eq!(thinking, "secret");
        assert_eq!(texts, "visible");
    }

    #[test]
    fn multiple_tool_calls_get_distinct_ids() {
        let mut p = ToolCallParser::new();
        let out = collect(
            &mut p,
            &[
                "<|tool_call>call:a{}<tool_call|>",
                "<|tool_call>call:b{}<tool_call|>",
            ],
        );
        let ids: Vec<&str> = out
            .iter()
            .filter_map(|o| {
                if let Output::ToolUse { tool_call_id, .. } = o {
                    Some(tool_call_id.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(ids, vec!["tc-1", "tc-2"]);
    }

    #[test]
    fn tool_call_with_int_value() {
        let mut p = ToolCallParser::new();
        let out = p.push("<|tool_call>call:set{n:42}<tool_call|>");
        match &out[0] {
            Output::ToolUse { input, .. } => {
                assert_eq!(input["n"], 42);
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_with_no_args() {
        let mut p = ToolCallParser::new();
        let out = p.push("<|tool_call>call:ping{}<tool_call|>");
        match &out[0] {
            Output::ToolUse { name, input, .. } => {
                assert_eq!(name, "ping");
                assert!(input.as_object().expect("object").is_empty());
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }
}
