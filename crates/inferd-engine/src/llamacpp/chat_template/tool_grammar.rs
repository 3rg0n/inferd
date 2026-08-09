//! GBNF building blocks for tool-call constrained decoding.
//!
//! [`ToolChoice`](inferd_proto::v2::ToolChoice) is only a *guarantee* if
//! something masks the tokens that would violate it. That something is a
//! grammar, and a grammar is per-family for the same reason the prompt is
//! (ADR 0026): `required` means "emit `<|tool_call>call:…<tool_call|>`"
//! for Gemma 4 and something else entirely for the next family. So the
//! grammar is produced by [`ChatRenderer::tool_call_grammar`], and this
//! module holds only the parts that are *not* family-specific.
//!
//! ## Why hand-written, and not `json_schema_to_gbnf`
//!
//! Issue #38 proposed reusing the shipped JSON-Schema→GBNF path. That
//! cannot work: Gemma 4's tool-call syntax is not JSON. It quotes strings
//! with the `<|"|>` control token, writes object keys bare, and wraps the
//! whole call in `<|tool_call>` / `<tool_call|>`. A JSON grammar would
//! mask every token the model actually needs to emit. Upstream reaches
//! the same conclusion — `common_chat_params_init_gemma4`
//! (`vendor/llama.cpp/common/chat.cpp`) hand-builds `gemma4-dict` &c.
//! rather than calling its own schema converter — and this module is
//! transposed from it.
//!
//! ## The one primitive worth explaining: exclusion
//!
//! Both `required` and `none` need a rule for *"any text that does not
//! contain S"* — `none` is exactly that language over `<|tool_call>`, and
//! `required` needs it as the prefix the model may fill with reasoning
//! before it commits to a call. GBNF has no "not this string" operator
//! (`[^…]` excludes single characters, not sequences), so
//! [`push_exclusion_rules`] emits the complement automaton for S as
//! right-recursive rules — the same construction upstream's PEG compiler
//! performs for `until(…)`, minus the multi-pattern Aho-Corasick
//! generality we do not need.

use std::fmt::Write as _;

/// The start symbol of every grammar this module builds.
pub const GRAMMAR_ROOT: &str = "root";

/// A constrained-decoding grammar for one family's tool-call syntax.
///
/// Produced on the calling task (it is pure string work) and handed to
/// the sampler-building code, which owns the FFI. Keeping the two apart
/// is what lets the whole grammar be unit-tested without a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolGrammar {
    /// GBNF source. Its start symbol is [`GRAMMAR_ROOT`].
    pub gbnf: String,
    /// Regex patterns that arm a *lazy* grammar: generation is
    /// unconstrained until one matches, then the grammar is fed the
    /// output from the match onward (`llama_grammar_accept_impl` replays
    /// the overlapping tokens). Empty means the grammar constrains every
    /// token from the first one.
    ///
    /// This is the difference between "shape the call if you make one"
    /// and "you must make a call": a lazy grammar cannot force anything,
    /// because until it triggers it masks nothing.
    pub triggers: Vec<String>,
}

impl ToolGrammar {
    /// Whether this grammar must be built with the lazy entry point.
    pub fn is_lazy(&self) -> bool {
        !self.triggers.is_empty()
    }
}

/// Escape one character for use inside a GBNF `"…"` literal or `[…]`
/// class.
///
/// `parse_char` (`vendor/llama.cpp/src/llama-grammar.cpp`) accepts only
/// `\x \u \U \t \r \n \\ \" \[ \]` — anything else after a backslash
/// throws "unknown escape". Notably there is **no** `\-`, which is why
/// [`push_negated_class`] positions `-` rather than escaping it.
fn escape_char(c: char) -> String {
    match c {
        '\\' => "\\\\".to_string(),
        '"' => "\\\"".to_string(),
        '[' => "\\[".to_string(),
        ']' => "\\]".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        // `\x` consumes exactly two hex digits, so this covers C0 and
        // DEL and nothing wider.
        c if (c as u32) < 0x20 || c as u32 == 0x7f => format!("\\x{:02x}", c as u32),
        c => c.to_string(),
    }
}

/// Escape a whole string for a GBNF `"…"` literal.
///
/// Used for tool names, which come from the request: a name carrying a
/// quote or a backslash must not be able to terminate the literal and
/// inject grammar. (A name that still produced unparseable GBNF would
/// surface as a NULL sampler, never as a throw across FFI — but not
/// relying on that is cheaper than reasoning about it.)
pub fn escape_literal(s: &str) -> String {
    s.chars().map(escape_char).collect()
}

/// Escape a string for the ECMAScript regex `std::regex` compiles for a
/// lazy grammar's trigger pattern.
///
/// Mirrors upstream `regex_escape` (`common/common.cpp`), which is what
/// `common_sampler` applies to a `COMMON_GRAMMAR_TRIGGER_TYPE_WORD`
/// trigger before handing it to `llama_sampler_init_grammar_lazy_patterns`.
/// Our triggers are control-token literals like `<|tool_call>`, whose `|`
/// is regex alternation if left unescaped — which would arm the grammar
/// on a bare `<`.
pub fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if matches!(
            c,
            '.' | '^' | '$' | '|' | '(' | ')' | '*' | '+' | '?' | '[' | ']' | '{' | '}' | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Emit `[^…]` excluding every character in `chars` (already sorted and
/// deduped by the caller).
fn push_negated_class(out: &mut String, chars: &[char]) {
    out.push_str("[^");
    // A `-` between two characters is a range separator and there is no
    // `\-` escape, so it goes last: the parser only reads `-` as a range
    // when the character after it is not `]`.
    for c in chars.iter().copied().filter(|c| *c != '-') {
        out.push_str(&escape_char(c));
    }
    if chars.contains(&'-') {
        out.push('-');
    }
    out.push(']');
}

/// KMP failure function: `f[i]` is the length of the longest proper
/// prefix of `pat[..=i]` that is also a suffix of it.
fn kmp_failure(pat: &[char]) -> Vec<usize> {
    let mut f = vec![0usize; pat.len()];
    let mut k = 0;
    for i in 1..pat.len() {
        while k > 0 && pat[i] != pat[k] {
            k = f[k - 1];
        }
        if pat[i] == pat[k] {
            k += 1;
        }
        f[i] = k;
    }
    f
}

/// One transition of the complement automaton for `pat`: from `state`
/// (the number of characters of `pat` matched so far, `state < pat.len()`)
/// on character `c`.
///
/// Returns `pat.len()` when the transition *completes* `pat` — the edge
/// the emitted grammar omits, which is the whole mechanism: a token that
/// would finish the forbidden string has no surviving stack, so
/// `llama_grammar_reject_candidates` masks it to `-inf`.
///
/// `pub(crate)` so the tests can walk this table against a plain
/// `str::contains` oracle, which is the only way to check the emitted
/// rules without a live vocab.
pub(crate) fn exclusion_step(pat: &[char], fail: &[usize], state: usize, c: char) -> usize {
    let mut s = state;
    loop {
        if pat[s] == c {
            return s + 1;
        }
        if s == 0 {
            return 0;
        }
        s = fail[s - 1];
    }
}

/// Emit rules matching *any text that does not contain `needle`*, named
/// `{prefix}0` … `{prefix}{n-1}`, one per automaton state. The entry
/// symbol is `{prefix}0`.
///
/// Every state is accepting — a partial match of `needle` is still text
/// that does not contain it — so each rule wraps its alternatives in
/// `(…)?`. That nullability is also what keeps EOG reachable:
/// `llama_grammar_apply_impl` only allows an end-of-generation token
/// while some stack is empty, so a rule that could not terminate would
/// forbid the model from ever ending its turn.
///
/// Panics if `needle` is empty (no caller can supply one; the excluded
/// strings are compile-time control-token constants).
pub fn push_exclusion_rules(out: &mut String, prefix: &str, needle: &str) {
    let pat: Vec<char> = needle.chars().collect();
    assert!(!pat.is_empty(), "cannot exclude the empty string");
    let fail = kmp_failure(&pat);

    let mut alphabet = pat.clone();
    alphabet.sort_unstable();
    alphabet.dedup();

    for state in 0..pat.len() {
        let _ = write!(out, "{prefix}{state} ::= (");
        let mut first = true;
        for c in alphabet.iter().copied() {
            let next = exclusion_step(&pat, &fail, state, c);
            if next == pat.len() {
                // The forbidden edge. Omitted, not redirected: the point
                // is that this character has nowhere to go from here.
                continue;
            }
            if !first {
                out.push_str(" | ");
            }
            first = false;
            let _ = write!(out, "\"{}\" {prefix}{next}", escape_char(c));
        }
        // A character the needle does not contain cannot extend any
        // partial match, so it always returns to state 0.
        if !first {
            out.push_str(" | ");
        }
        push_negated_class(out, &alphabet);
        let _ = writeln!(out, " {prefix}0)?");
    }
}

/// Check that every rule referenced by `gbnf` is defined, that no rule
/// is defined twice, and that `root` exists.
///
/// Not a GBNF parser — llama.cpp owns that, and reimplementing it here
/// would drift. This catches the one failure mode hand-written grammars
/// actually hit: a misspelled rule name. Upstream reports that as
/// `llama_grammar_init_impl` returning `nullptr`, which inferd maps to
/// `LlamaCppError::Sampler` — a runtime error, on a path that only
/// Tier 3 exercises and Tier 3 is in no default gate. Running this in a
/// unit test moves the same defect to `cargo test`.
///
/// Test-only: the shipped grammars are built from constants, so there
/// is nothing to validate at runtime that is not already fixed at
/// compile time.
#[cfg(test)]
pub(crate) fn check_rule_closure(gbnf: &str, root: &str) -> Result<(), String> {
    let mut defined: Vec<&str> = Vec::new();
    let mut referenced: Vec<(&str, usize)> = Vec::new();

    for (lineno, line) in gbnf.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((lhs, rhs)) = line.split_once("::=") else {
            return Err(format!("line {}: no `::=` in {line:?}", lineno + 1));
        };
        let name = lhs.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            // `_` is not a GBNF word character (`is_word_char` is
            // `[a-zA-Z0-9-]`), so a rule named with one would parse as
            // a truncated name followed by garbage.
            return Err(format!("line {}: bad rule name {name:?}", lineno + 1));
        }
        if defined.contains(&name) {
            return Err(format!("line {}: rule {name:?} defined twice", lineno + 1));
        }
        defined.push(name);
        referenced.extend(rule_refs(rhs).into_iter().map(|r| (r, lineno + 1)));
    }

    if !defined.contains(&root) {
        return Err(format!("root rule {root:?} is not defined"));
    }
    for (r, lineno) in referenced {
        if !defined.contains(&r) {
            return Err(format!("line {lineno}: undefined rule {r:?}"));
        }
    }
    Ok(())
}

/// Extract rule references from the right-hand side of one GBNF rule:
/// bare identifiers outside string literals and character classes.
#[cfg(test)]
fn rule_refs(rhs: &str) -> Vec<&str> {
    let bytes = rhs.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                // Skip the literal, honouring `\"`.
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'[' => {
                // Skip the class, honouring `\]`.
                i += 1;
                while i < bytes.len() && bytes[i] != b']' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            c if c.is_ascii_alphabetic() => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_')
                {
                    i += 1;
                }
                out.push(&rhs[start..i]);
            }
            _ => i += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walk the emitted automaton over `input`. Returns `false` as soon
    /// as a character has no transition — i.e. exactly when the grammar
    /// would mask it.
    fn accepts(needle: &str, input: &str) -> bool {
        let pat: Vec<char> = needle.chars().collect();
        let fail = kmp_failure(&pat);
        let mut state = 0usize;
        for c in input.chars() {
            let next = exclusion_step(&pat, &fail, state, c);
            if next == pat.len() {
                return false;
            }
            state = next;
        }
        true
    }

    #[test]
    fn exclusion_automaton_agrees_with_contains() {
        // The oracle: the emitted rules must accept a string iff it does
        // not contain the needle. Cases chosen to hit the traps —
        // repeated first characters, near-misses, the needle straddling
        // earlier partial matches, and the needle at each position.
        let needle = "<|tool_call>";
        let cases = [
            "",
            "hello",
            "<",
            "<<<",
            "<|",
            "<|tool_cal",
            "<|tool_call",
            "<|tool_call>",
            "text <|tool_call> more",
            "<|tool<|tool_call>",
            "<|channel>thought\nI should call it<channel|>",
            "<tool_call|>",
            "<|TOOL_CALL>",
            "a<|tool_call>b",
        ];
        for case in cases {
            assert_eq!(
                accepts(needle, case),
                !case.contains(needle),
                "mismatch on {case:?}"
            );
        }
    }

    #[test]
    fn exclusion_handles_a_needle_with_a_real_border() {
        // `<|"|>` has no proper prefix that is also a suffix, so it never
        // exercises the KMP fallback. `aab` does not either; `aba` does:
        // after "ab" a mismatching 'a' must fall back to state 1, not 0,
        // or "abab a" style inputs are misjudged.
        let needle = "aba";
        for case in [
            "", "a", "ab", "aba", "abab", "abba", "aabaa", "ababa", "abb",
        ] {
            assert_eq!(
                accepts(needle, case),
                !case.contains(needle),
                "mismatch on {case:?}"
            );
        }
    }

    #[test]
    fn exclusion_of_the_gemma_string_fence() {
        let needle = "<|\"|>";
        for case in [
            "", "plain", "<|", "<|\"", "<|\"|", "<|\"|>", "a<|\"|>b", "<<|\"|>",
        ] {
            assert_eq!(
                accepts(needle, case),
                !case.contains(needle),
                "mismatch on {case:?}"
            );
        }
    }

    #[test]
    fn single_character_needle_yields_a_class_only_rule() {
        let mut g = String::new();
        push_exclusion_rules(&mut g, "s", "x");
        // Every edge out of state 0 on 'x' completes the needle, so only
        // the catch-all class survives.
        assert_eq!(g, "s0 ::= ([^x] s0)?\n");
    }

    #[test]
    fn emitted_rules_are_one_per_state_and_start_at_zero() {
        let mut g = String::new();
        push_exclusion_rules(&mut g, "pre", "<|tool_call>");
        assert_eq!(g.lines().count(), "<|tool_call>".len());
        assert!(g.starts_with("pre0 ::= ("));
        assert!(g.contains("pre11 ::= ("));
        // Rule names must be GBNF words ([a-zA-Z0-9-]); an underscore
        // would terminate the name and produce a parse error.
        assert!(
            !g.lines()
                .any(|l| l.split_whitespace().next().unwrap().contains('_'))
        );
    }

    #[test]
    fn dash_in_the_alphabet_lands_where_it_cannot_form_a_range() {
        // `[^-a]` and `[^a-]` are both literal; `[^a-z]` is a range. The
        // emitted class must never be the third.
        let mut g = String::new();
        push_exclusion_rules(&mut g, "s", "a-z");
        assert!(g.contains("[^az-]"), "got: {g}");
    }

    #[test]
    fn quote_and_backslash_are_escaped_in_literals() {
        assert_eq!(escape_literal("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_literal("a\\b"), "a\\\\b");
        assert_eq!(escape_literal("tab\there"), "tab\\there");
        assert_eq!(escape_literal("\u{1}"), "\\x01");
        // The common case must pass through untouched.
        assert_eq!(escape_literal("get_weather"), "get_weather");
    }

    #[test]
    fn regex_escape_neutralises_the_control_token_pipe() {
        // Unescaped, `<|tool_call>` is the alternation `<` or `tool_call>`,
        // so the grammar would arm on the first `<` the model emits.
        assert_eq!(regex_escape("<|tool_call>"), "<\\|tool_call>");
        assert_eq!(regex_escape("a.b*c"), "a\\.b\\*c");
    }

    #[test]
    fn closure_check_accepts_a_well_formed_grammar() {
        let g = "root ::= a b\na ::= \"x\" | c\nb ::= [a-z]+\nc ::= \"y\"\n";
        assert_eq!(check_rule_closure(g, "root"), Ok(()));
    }

    #[test]
    fn closure_check_catches_the_typo_it_exists_for() {
        // The real failure mode: a reference that does not resolve.
        // Upstream turns this into a NULL sampler at generation time.
        let g = "root ::= gemma4-dict\ngemma4-dcit ::= \"{}\"\n";
        let err = check_rule_closure(g, "root").unwrap_err();
        assert!(err.contains("undefined rule"), "got: {err}");
        assert!(err.contains("gemma4-dict"), "got: {err}");
    }

    #[test]
    fn closure_check_ignores_names_inside_literals_and_classes() {
        // `root` appears inside a literal and a class here; neither is a
        // rule reference, and treating them as one would make every
        // grammar containing the word fail.
        let g = "root ::= \"undefined-rule\" [a-z] \"a::=b\"\n";
        assert_eq!(check_rule_closure(g, "root"), Ok(()));
    }

    #[test]
    fn closure_check_rejects_a_missing_root_and_a_duplicate() {
        assert!(
            check_rule_closure("a ::= \"x\"\n", "root")
                .unwrap_err()
                .contains("root")
        );
        assert!(
            check_rule_closure("root ::= \"x\"\nroot ::= \"y\"\n", "root")
                .unwrap_err()
                .contains("twice")
        );
    }

    #[test]
    fn closure_check_rejects_an_underscore_rule_name() {
        // GBNF's `is_word_char` is `[a-zA-Z0-9-]`, so `gemma4_dict`
        // parses as `gemma4` followed by junk. Cheap to write by
        // accident when every other Rust identifier uses `_`.
        let err = check_rule_closure("root ::= x\nx ::= \"a\"\ngemma4_dict ::= \"b\"\n", "root")
            .unwrap_err();
        assert!(err.contains("bad rule name"), "got: {err}");
    }

    #[test]
    fn exclusion_rules_are_self_contained() {
        let mut g = String::from("root ::= pre-0\n");
        push_exclusion_rules(&mut g, "pre-", "<|tool_call>");
        assert_eq!(check_rule_closure(&g, "root"), Ok(()));
    }

    #[test]
    fn lazy_is_decided_by_the_trigger_list() {
        let lazy = ToolGrammar {
            gbnf: String::new(),
            triggers: vec!["x".into()],
        };
        let eager = ToolGrammar {
            gbnf: String::new(),
            triggers: vec![],
        };
        assert!(lazy.is_lazy());
        assert!(!eager.is_lazy());
    }
}
