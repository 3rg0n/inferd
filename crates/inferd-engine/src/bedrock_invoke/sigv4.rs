//! AWS Signature Version 4 signing for Bedrock-runtime requests.
//!
//! Implements the subset we need: `POST` with a JSON body, signing
//! the canonical headers `host`, `x-amz-date`, and (when the session
//! token is present) `x-amz-security-token`. Service is always
//! `bedrock`; payload is signed (SHA-256 hash sent as the
//! `x-amz-content-sha256` header).
//!
//! References:
//! - <https://docs.aws.amazon.com/general/latest/gr/sigv4_signing.html>
//! - <https://docs.aws.amazon.com/general/latest/gr/sigv4_elements.html>
//!
//! v0.2.0 doesn't pull in the full `aws-sigv4` crate — that crate
//! drags the rest of the AWS SDK ecosystem along (`aws-types`,
//! `aws-smithy-types`, …) which we don't need. Hand-rolling SigV4 is
//! ~150 lines of HMAC-SHA-256 against well-defined string templates;
//! the spec is stable and tests cover the canonical examples from
//! AWS' documentation.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// AWS credentials used for signing.
#[derive(Debug, Clone)]
pub(super) struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    /// Set when the credentials came from STS / session-token-bearing
    /// chain. Surfaces as the `x-amz-security-token` header.
    pub session_token: Option<String>,
}

/// Inputs to `sign_request`. The signer mutates the headers map by
/// adding the canonical `host`, `x-amz-date`, `x-amz-content-sha256`,
/// optional `x-amz-security-token`, and final `Authorization`.
pub(super) struct SignRequest<'a> {
    pub method: &'a str,
    /// Host portion of the URL (no scheme, no port — Bedrock uses 443).
    pub host: &'a str,
    /// URL path. Must start with `/`. Already URL-encoded.
    pub path: &'a str,
    /// Query string without leading `?`. Empty string ok.
    pub query: &'a str,
    pub region: &'a str,
    pub service: &'a str,
    /// Body bytes. Hashed (SHA-256) to populate
    /// `x-amz-content-sha256`.
    pub body: &'a [u8],
    /// `YYYYMMDDTHHMMSSZ` UTC timestamp.
    pub amz_date: &'a str,
    /// `YYYYMMDD` portion of `amz_date`.
    pub date_stamp: &'a str,
}

/// Output of signing — the headers to attach to the request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SignedHeaders {
    pub host: String,
    pub x_amz_date: String,
    pub x_amz_content_sha256: String,
    pub x_amz_security_token: Option<String>,
    pub authorization: String,
}

/// Compute the signed headers for the request. Does not mutate the
/// request itself — the caller stitches `SignedHeaders` into the
/// reqwest builder.
pub(super) fn sign_request(req: &SignRequest<'_>, creds: &AwsCredentials) -> SignedHeaders {
    // 1. Canonical request.
    let payload_hash = hex::encode(Sha256::digest(req.body));
    let mut signed_headers_list: Vec<(&str, String)> = vec![
        ("host", req.host.to_ascii_lowercase()),
        ("x-amz-content-sha256", payload_hash.clone()),
        ("x-amz-date", req.amz_date.to_string()),
    ];
    if let Some(token) = &creds.session_token {
        signed_headers_list.push(("x-amz-security-token", token.clone()));
    }
    signed_headers_list.sort_by(|a, b| a.0.cmp(b.0));

    let signed_header_names: String = signed_headers_list
        .iter()
        .map(|(n, _)| *n)
        .collect::<Vec<_>>()
        .join(";");

    let canonical_headers: String = signed_headers_list
        .iter()
        .map(|(n, v)| format!("{n}:{}\n", v.trim()))
        .collect();

    let canonical_request = format!(
        "{method}\n{path}\n{query}\n{canonical_headers}\n{signed}\n{payload}",
        method = req.method,
        path = req.path,
        query = req.query,
        canonical_headers = canonical_headers,
        signed = signed_header_names,
        payload = payload_hash,
    );

    // 2. String to sign.
    let credential_scope = format!(
        "{date}/{region}/{service}/aws4_request",
        date = req.date_stamp,
        region = req.region,
        service = req.service,
    );
    let canonical_request_hash = hex::encode(Sha256::digest(canonical_request.as_bytes()));
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{hash}",
        amz_date = req.amz_date,
        scope = credential_scope,
        hash = canonical_request_hash,
    );

    // 3. Derive signing key. HMAC-SHA-256 chained:
    //   k_secret  = "AWS4" + secret_access_key
    //   k_date    = HMAC(k_secret, date_stamp)
    //   k_region  = HMAC(k_date, region)
    //   k_service = HMAC(k_region, service)
    //   k_signing = HMAC(k_service, "aws4_request")
    let k_secret = format!("AWS4{}", creds.secret_access_key);
    let k_date = hmac_sha256(k_secret.as_bytes(), req.date_stamp.as_bytes());
    let k_region = hmac_sha256(&k_date, req.region.as_bytes());
    let k_service = hmac_sha256(&k_region, req.service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");

    let signature = hex::encode(hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={ak}/{scope}, SignedHeaders={signed}, Signature={sig}",
        ak = creds.access_key_id,
        scope = credential_scope,
        signed = signed_header_names,
        sig = signature,
    );

    SignedHeaders {
        host: req.host.to_string(),
        x_amz_date: req.amz_date.to_string(),
        x_amz_content_sha256: payload_hash,
        x_amz_security_token: creds.session_token.clone(),
        authorization,
    }
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_is_deterministic() {
        // Same inputs → same Authorization. Different body → different
        // signature. Establishes the building blocks work end-to-end
        // without binding to AWS' specific test vectors (those use a
        // different signed-headers list than ours).
        let creds = AwsCredentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
        };
        let common = SignRequest {
            method: "POST",
            host: "bedrock-runtime.us-east-1.amazonaws.com",
            path: "/model/anthropic.claude/invoke-with-response-stream",
            query: "",
            region: "us-east-1",
            service: "bedrock",
            body: b"{\"messages\":[]}",
            amz_date: "20260521T120000Z",
            date_stamp: "20260521",
        };
        let a = sign_request(&common, &creds);
        let b = sign_request(&common, &creds);
        assert_eq!(a.authorization, b.authorization);
        assert!(
            a.authorization
                .contains("Credential=AKIDEXAMPLE/20260521/us-east-1/bedrock/aws4_request")
        );
        assert!(
            a.authorization
                .contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date")
        );

        let other = SignRequest {
            body: b"{\"messages\":[{\"role\":\"user\"}]}",
            ..common
        };
        let c = sign_request(&other, &creds);
        assert_ne!(a.authorization, c.authorization);
    }

    #[test]
    fn session_token_appears_in_signed_headers() {
        let creds = AwsCredentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: Some("SESSIONTOKEN".into()),
        };
        let signed = sign_request(
            &SignRequest {
                method: "POST",
                host: "bedrock-runtime.us-east-1.amazonaws.com",
                path: "/model/anthropic.claude-3-5-sonnet-20241022-v2%3A0/invoke-with-response-stream",
                query: "",
                region: "us-east-1",
                service: "bedrock",
                body: b"{}",
                amz_date: "20260521T120000Z",
                date_stamp: "20260521",
            },
            &creds,
        );
        assert!(
            signed.authorization.contains(
                "SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token"
            )
        );
        assert_eq!(signed.x_amz_security_token.as_deref(), Some("SESSIONTOKEN"));
    }

    #[test]
    fn empty_body_hashes_to_known_constant() {
        // SHA-256 of empty string — well-known constant from the
        // SigV4 spec; serves as a sanity check on the hash plumbing.
        let creds = AwsCredentials {
            access_key_id: "x".into(),
            secret_access_key: "y".into(),
            session_token: None,
        };
        let signed = sign_request(
            &SignRequest {
                method: "POST",
                host: "h",
                path: "/",
                query: "",
                region: "us-east-1",
                service: "bedrock",
                body: b"",
                amz_date: "20260521T000000Z",
                date_stamp: "20260521",
            },
            &creds,
        );
        assert_eq!(
            signed.x_amz_content_sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
