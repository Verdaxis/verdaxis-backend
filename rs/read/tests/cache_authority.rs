use verdaxis_read::runtime::cache_authority::{Authority, CacheAuthority};

fn identity(epoch: &str) -> Authority {
    Authority {
        database_epoch: epoch.into(),
        primary_identity: "primary".into(),
        revision: 1,
    }
}

#[test]
fn unknown_authority_and_reused_generation_never_authorize_hits() {
    let authority = CacheAuthority::default();
    assert!(authority.token(&identity("a")).is_none());
    authority.activate(identity("a")).unwrap();
    let fill = authority.token(&identity("a")).unwrap();
    assert!(authority.accepts(&fill));
    authority.revoke();
    assert!(!authority.accepts(&fill));
    authority.activate(identity("b")).unwrap();
    assert!(!authority.accepts(&fill));
    assert!(authority.token(&identity("a")).is_none());
}

#[test]
fn reconnect_fences_prior_fills_even_with_same_database_identity() {
    let authority = CacheAuthority::default();
    authority.activate(identity("a")).unwrap();
    let fill = authority.token(&identity("a")).unwrap();
    authority.revoke();
    authority.activate(identity("a")).unwrap();
    assert!(!authority.accepts(&fill));
}
