//! The client library's default endpoints must name the sockets the
//! daemon actually binds.
//!
//! `inferd-client` cannot depend on `inferd-daemon` (it is the published,
//! dependency-light library), so it carries its own copy of the default
//! path chain. This crate links both, which makes it the one place the
//! two can be compared: a drift here is a consumer that connects to a
//! path nothing listens on.

#[test]
fn client_defaults_match_daemon_defaults() {
    // Same process, same environment, so both sides read the same
    // XDG_RUNTIME_DIR / HOME / TMPDIR.
    assert_eq!(
        inferd_client::default_v2_addr(),
        inferd_daemon::endpoint::default_addr(),
        "generation"
    );
    assert_eq!(
        inferd_client::default_embed_addr(),
        inferd_daemon::endpoint::default_embed_addr(),
        "embed"
    );
    assert_eq!(
        inferd_client::default_rerank_addr(),
        inferd_daemon::endpoint::default_rerank_addr(),
        "rerank"
    );
    assert_eq!(
        inferd_client::default_admin_addr(),
        inferd_daemon::endpoint::default_admin_addr(),
        "admin"
    );
}
