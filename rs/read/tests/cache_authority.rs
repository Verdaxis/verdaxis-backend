use std::time::{Duration, Instant};
use verdaxis_read::runtime::cache_authority::{
    AUTHORITY_LEASE, Authority, CacheAuthority, FillTicket, parse_record, read_record,
};

fn identity(seed: char) -> Authority {
    Authority {
        database_epoch: seed.to_string().repeat(32),
        primary_identity: "b".repeat(32),
        revision: 1,
    }
}

/// An authority that is active and has its listener attached, as at runtime.
fn serving(authority: &CacheAuthority, database: &Authority, now: Instant) {
    authority.synchronize(Some(database.clone()), now).unwrap();
    assert!(authority.attach_listener(database).is_some());
}

#[test]
fn unknown_authority_and_reused_generation_never_authorize_hits() {
    let now = Instant::now();
    let authority = CacheAuthority::default();
    assert!(authority.token(&identity('a'), now).is_none());
    serving(&authority, &identity('a'), now);
    let fill = authority.token(&identity('a'), now).unwrap();
    assert!(authority.accepts(&fill, now));
    authority.revoke();
    assert!(!authority.accepts(&fill, now));
    serving(&authority, &identity('c'), now);
    assert!(!authority.accepts(&fill, now));
    assert!(authority.token(&identity('a'), now).is_none());
}

#[test]
fn reconnect_fences_prior_fills_even_with_same_database_identity() {
    let now = Instant::now();
    let authority = CacheAuthority::default();
    serving(&authority, &identity('a'), now);
    let fill = authority.token(&identity('a'), now).unwrap();
    authority.revoke();
    serving(&authority, &identity('a'), now);
    assert!(!authority.accepts(&fill, now));
}

#[test]
fn database_identity_must_equal_the_external_record() {
    let now = Instant::now();
    let authority = CacheAuthority::default();
    serving(&authority, &identity('a'), now);
    let mut other_revision = identity('a');
    other_revision.revision = 2;
    let mut other_primary = identity('a');
    other_primary.primary_identity = "d".repeat(32);
    for database in [identity('c'), other_revision, other_primary] {
        assert!(authority.token(&database, now).is_none());
    }
    assert!(authority.token(&identity('a'), now).is_some());
}

#[test]
fn hits_need_a_listener_bound_to_the_same_identity() {
    let now = Instant::now();
    let authority = CacheAuthority::default();
    authority.synchronize(Some(identity('a')), now).unwrap();
    assert!(authority.token(&identity('a'), now).is_none());
    // A listener that read another database's identity cannot attach.
    assert!(authority.attach_listener(&identity('c')).is_none());
    let listener = authority.attach_listener(&identity('a')).unwrap();
    let token = authority.token(&identity('a'), now).unwrap();
    authority.detach_listener(listener);
    assert!(authority.token(&identity('a'), now).is_none());
    assert!(!authority.accepts(&token, now));
}

#[test]
fn replaced_listener_cannot_detach_its_successor() {
    let now = Instant::now();
    let authority = CacheAuthority::default();
    authority.synchronize(Some(identity('a')), now).unwrap();
    let old = authority.attach_listener(&identity('a')).unwrap();
    let new = authority.attach_listener(&identity('a')).unwrap();
    assert!(!authority.listener_is_current(old));
    authority.detach_listener(old);
    assert!(authority.listener_is_current(new));
    assert!(authority.token(&identity('a'), now).is_some());
}

#[test]
fn a_new_external_identity_drops_the_old_listener_and_fences_tokens() {
    let now = Instant::now();
    let authority = CacheAuthority::default();
    serving(&authority, &identity('a'), now);
    let fill = authority.token(&identity('a'), now).unwrap();
    let fence = authority.current_fence();
    authority.synchronize(Some(identity('c')), now).unwrap();
    assert_ne!(authority.current_fence(), fence);
    assert!(!authority.accepts(&fill, now));
    // The successor has no listener yet, so it serves no hit until one attaches.
    assert!(authority.token(&identity('c'), now).is_none());
    assert!(authority.attach_listener(&identity('c')).is_some());
    assert!(authority.token(&identity('c'), now).is_some());
}

#[test]
fn renewing_the_same_record_keeps_tokens_and_fence() {
    let now = Instant::now();
    let authority = CacheAuthority::default();
    serving(&authority, &identity('a'), now);
    let fill = authority.token(&identity('a'), now).unwrap();
    let fence = authority.current_fence();
    authority
        .synchronize(Some(identity('a')), now + Duration::from_secs(1))
        .unwrap();
    assert_eq!(authority.current_fence(), fence);
    assert!(authority.accepts(&fill, now + Duration::from_secs(1)));
}

#[test]
fn an_expired_lease_serves_no_hit_until_the_record_is_read_again() {
    let start = Instant::now();
    let authority = CacheAuthority::default();
    serving(&authority, &identity('a'), start);
    let fill = authority.token(&identity('a'), start).unwrap();
    let inside = start + AUTHORITY_LEASE - Duration::from_millis(1);
    let outside = start + AUTHORITY_LEASE;
    assert!(authority.accepts(&fill, inside));
    assert!(authority.token(&identity('a'), inside).is_some());
    assert!(!authority.accepts(&fill, outside));
    assert!(authority.token(&identity('a'), outside).is_none());
    authority.synchronize(Some(identity('a')), outside).unwrap();
    assert!(authority.token(&identity('a'), outside).is_some());
}

#[test]
fn unproved_records_clear_authority() {
    let now = Instant::now();
    let authority = CacheAuthority::default();
    serving(&authority, &identity('a'), now);
    let mut unproved = identity('c');
    unproved.revision = 0;
    assert!(authority.synchronize(Some(unproved), now).is_err());
    assert!(authority.token(&identity('a'), now).is_none());
    serving(&authority, &identity('a'), now);
    authority.synchronize(None, now).unwrap();
    assert!(authority.token(&identity('a'), now).is_none());
}

#[test]
fn a_fill_publishes_only_under_its_own_fence_and_latest_generation() {
    let now = Instant::now();
    let authority = CacheAuthority::default();
    serving(&authority, &identity('a'), now);
    let token = authority.token(&identity('a'), now).unwrap();
    let ticket = FillTicket::new(token, 100);
    assert!(authority.may_publish(&ticket, 100, now));
    assert!(!authority.may_publish(&ticket, 101, now));
    assert!(!authority.may_publish(&ticket, 99, now));
    authority.revoke();
    assert!(!authority.may_publish(&ticket, 100, now));
}

#[test]
fn record_vectors_are_the_shared_cross_language_contract() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("data/cache_authority_records.json")).unwrap();
    for case in vectors["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let text = match case.get("raw") {
            Some(raw) => raw.as_str().unwrap().to_owned(),
            None => serde_json::to_string(&case["record"]).unwrap(),
        };
        let parsed = parse_record(&text);
        if case["accept"].as_bool().unwrap() {
            let expected = match &case["authority"] {
                serde_json::Value::Null => None,
                value => Some(Authority {
                    database_epoch: value["database_epoch"].as_str().unwrap().to_owned(),
                    primary_identity: value["primary_identity"].as_str().unwrap().to_owned(),
                    revision: value["revision"].as_u64().unwrap(),
                }),
            };
            assert_eq!(parsed, Ok(expected), "{name}");
        } else {
            assert!(parsed.is_err(), "{name}");
        }
    }
}

#[test]
fn record_files_must_be_small_regular_files() {
    let directory = std::env::temp_dir().join(format!("verdaxis-authority-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let record = directory.join("cache-authority.json");
    let active = format!(
        "{{\"version\":1,\"state\":\"active\",\"database_epoch\":\"{}\",\"primary_identity\":\"{}\",\"authority_revision\":2}}",
        "a".repeat(32),
        "b".repeat(32)
    );
    assert!(read_record(&record).is_err());
    std::fs::write(&record, &active).unwrap();
    assert_eq!(read_record(&record).unwrap().unwrap().revision, 2);
    std::fs::write(&record, format!("{active}{}", " ".repeat(5000))).unwrap();
    assert!(read_record(&record).is_err());
    std::fs::remove_file(&record).unwrap();
    let real = directory.join("real.json");
    std::fs::write(&real, &active).unwrap();
    std::os::unix::fs::symlink(&real, &record).unwrap();
    assert!(read_record(&record).is_err());
    // refresh() turns every unreadable record into a disabled authority.
    let now = Instant::now();
    let authority = CacheAuthority::default();
    authority.synchronize(Some(identity('a')), now).unwrap();
    authority.refresh(&record, now);
    assert!(authority.attach_listener(&identity('a')).is_none());
    std::fs::remove_dir_all(&directory).unwrap();
}
