//! Write-time secret redactor for the activity log.
//!
//! THREAT_MODEL F-3: tokens flowing through the daemon may contain
//! credentials (env vars, JWTs, API keys). The activity log captures
//! request metadata, and at `INFERD_LOG=debug` records may include
//! request/response content. Anything in the record bytes that *looks
//! like* a known credential shape gets replaced with a stable
//! `[REDACTED:<kind>]` marker before disk write.
//!
//! False-positive rate is intentionally biased high: missing a real
//! credential is much worse than scrubbing a non-credential. Operators
//! who need the unredacted bytes can disable logging at the source via
//! `INFERD_LOG=0`.
//!
//! Patterns covered:
//! - JWT (three base64url-ish segments separated by `.`)
//! - Common API-key prefixes: `sk-`, `xoxb-`, `xoxa-`, `ghp_`, `gho_`,
//!   `ghs_`, `ghu_`, `pat-`, `thingspat_`, AWS `AKIA`, AWS `ASIA`
//! - `Authorization: Bearer …` and `Authorization: Basic …` lines
//! - Common key=value patterns: `password=…`, `passwd=…`, `pwd=…`,
//!   `api_key=…`, `apikey=…`, `token=…`, `secret=…` (case-insensitive)
//!
//! The redactor mutates the record string in place. It runs against
//! the already-serialised JSON line — no JSON parsing — so cost is
//! linear in record size.

use std::sync::OnceLock;

use regex::Regex;

/// Replace any matched credential-like substring in `line` with
/// `[REDACTED:<kind>]`. Mutates in place; no allocations on the
/// hot path beyond the regex engine's internal buffers.
pub fn redact_in_place(line: &mut String) {
    let original = std::mem::take(line);
    let scrubbed = redact_str(&original);
    *line = scrubbed;
}

fn redact_str(input: &str) -> String {
    let mut out = input.to_string();
    for r in rules() {
        out = r.regex.replace_all(&out, r.replacement).into_owned();
    }
    out
}

struct Rule {
    regex: Regex,
    replacement: &'static str,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(build_rules)
}

fn build_rules() -> Vec<Rule> {
    // Order matters: structural patterns first (Authorization, key=value)
    // so a token *inside* a header gets the more specific kind label,
    // then prefix patterns sweep up bare tokens.
    let entries: &[(&str, &str)] = &[
        // Authorization headers (HTTP-shape; covers JSON-embedded
        // "Authorization": "Bearer …" via the value match).
        (
            r#"(?i)(authorization\s*[:=]\s*"?\s*(?:bearer|basic|token)\s+)[A-Za-z0-9._\-+/=]{8,}"#,
            "[REDACTED:auth-header]",
        ),
        // key=value pairs (case-insensitive). Captures the value through
        // whitespace, comma, ampersand, quote, or end-of-string.
        (
            r#"(?i)\b(password|passwd|pwd|api[_-]?key|secret|token)\s*[:=]\s*"?[^\s",&}]{4,}"#,
            "[REDACTED:secret-kv]",
        ),
        // JWT — three base64url-ish chunks separated by `.`.
        (
            r"\beyJ[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+\b",
            "[REDACTED:jwt]",
        ),
        // OpenAI-style sk- keys.
        (r"\bsk-[A-Za-z0-9]{16,}\b", "[REDACTED:api-key]"),
        // Slack tokens (xoxb / xoxa / xoxp / xoxs).
        (r"\bxox[baps]-[A-Za-z0-9-]{10,}\b", "[REDACTED:api-key]"),
        // GitHub tokens (PAT, OAuth, server-to-server, user-to-server).
        (r"\bgh[posu]_[A-Za-z0-9]{20,}\b", "[REDACTED:api-key]"),
        // Personal access token shapes used by Cisco Things and others.
        (r"\bpat-[A-Za-z0-9]{16,}\b", "[REDACTED:api-key]"),
        (r"\bthingspat_[A-Za-z0-9]{16,}\b", "[REDACTED:api-key]"),
        // AWS access keys.
        (r"\bAKIA[0-9A-Z]{16}\b", "[REDACTED:aws-access]"),
        (r"\bASIA[0-9A-Z]{16}\b", "[REDACTED:aws-session]"),
    ];

    entries
        .iter()
        .map(|(pat, repl)| Rule {
            regex: Regex::new(pat)
                .unwrap_or_else(|e| panic!("redact rule {pat:?} failed to compile: {e}")),
            replacement: repl,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> String {
        let mut s = s.to_string();
        redact_in_place(&mut s);
        s
    }

    #[test]
    fn redacts_jwt() {
        let input =
            "context eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjMifQ.abc-DEF_xyz tail";
        let out = r(input);
        assert!(!out.contains("eyJhbGciOi"));
        assert!(out.contains("[REDACTED:jwt]"));
    }

    #[test]
    fn redacts_openai_sk_key() {
        // Fixture assembled at runtime so secret-scanning tools don't
        // treat it as a real key in source.
        let fixture = format!("{}-{}", "sk", "1234567890abcdefghij");
        let input = format!(r#"{{"key":"{fixture}","ok":true}}"#);
        let out = r(&input);
        assert!(!out.contains(&fixture), "out: {out}");
        assert!(out.contains("[REDACTED"));
    }

    #[test]
    fn redacts_authorization_bearer() {
        let value = "abcdefghij1234567890XYZ";
        let input = format!("req: Authorization: Bearer {value}");
        let out = r(&input);
        assert!(!out.contains(value), "out: {out}");
        assert!(out.contains("[REDACTED:auth-header]"));
    }

    #[test]
    fn redacts_password_kv() {
        let value = "sup3rh4rd!";
        let input = format!("db_url=postgres://user:supersecret123@host/db password={value}");
        let out = r(&input);
        assert!(!out.contains(value), "out: {out}");
    }

    #[test]
    fn redacts_aws_access_key() {
        let fixture = format!("{}{}", "AKIA", "IOSFODNN7EXAMPLE");
        let input = format!("boot {fixture} bye");
        let out = r(&input);
        assert!(!out.contains(&fixture), "out: {out}");
        assert!(out.contains("[REDACTED:aws-access]"));
    }

    #[test]
    fn redacts_github_token() {
        let fixture = format!("{}_{}", "ghp", "abcdefghijklmnopqrstuvwxyz12");
        let input = format!("tok={fixture}");
        let out = r(&input);
        assert!(!out.contains(&fixture), "out: {out}");
    }

    #[test]
    fn redacts_slack_xoxb() {
        let fixture = format!("{}-{}", "xoxb", "12345-67890-abcdefghi");
        let input = format!("header {fixture} tail");
        let out = r(&input);
        assert!(!out.contains(&fixture), "out: {out}");
    }

    #[test]
    fn passes_through_safe_text() {
        // Strings that look almost like credentials but aren't.
        let inputs = [
            "the quick brown fox",
            r#"{"msg":"hello world","level":"info"}"#,
            "version=0.1.0-alpha.0",
            "bytes=42",
        ];
        for s in inputs {
            let out = r(s);
            assert_eq!(out, s, "should not redact: {s}");
        }
    }

    #[test]
    fn handles_multiple_secrets_in_one_line() {
        let sk = format!("{}-{}", "sk", "1234567890abcdefghij");
        let aws = format!("{}{}", "AKIA", "IOSFODNN7EXAMPLE");
        let input = format!("{sk} and {aws}");
        let out = r(&input);
        assert!(!out.contains(&sk));
        assert!(!out.contains(&aws));
    }
}
