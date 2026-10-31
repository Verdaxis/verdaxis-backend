//! PD4-AUD-1: every audit action string Rust writes must exist in Python's
//! canonical registry. The pin reads the registry source the same way the
//! Python `AUDIT_ACTIONS` frozenset is built: from the module constants.

use std::collections::BTreeSet;

const REGISTRY: &str = include_str!("../../../app/services/audit_actions.py");

fn registry_actions() -> BTreeSet<String> {
    // The module defines each action as `NAME = "action.string"`; the
    // frozenset below repeats the same constants, so parsing the assignment
    // lines captures exactly the registered actions.
    let mut actions = BTreeSet::new();
    for line in REGISTRY.lines() {
        let Some((_, value)) = line.split_once("= \"") else {
            continue;
        };
        let Some(action) = value.strip_suffix('"') else {
            continue;
        };
        if !action.is_empty() && action.contains('.') {
            actions.insert(action.to_owned());
        }
    }
    actions
}

#[test]
fn every_rust_audit_action_is_registered_in_python() {
    let registered = registry_actions();
    // Every `action:` literal or conditional pair in the merged Phase 4 tree,
    // including the runtime `&str` passed by the invalidation audit helper.
    let written = [
        "admin.user_approved",
        "admin.user_rejected",
        "admin.organization_approved",
        "admin.organization_rejected",
        "organization.join_approved",
        "organization.join_rejected",
        "kyc.membership_invalidated",
        "kyc.submitted",
        "kyc.document_viewed",
        "kyc.approved",
        "kyc.rejected",
        "subscription.updated",
        "order.cancelled",
        "market.access_invalidated",
    ];
    let mut missing = Vec::new();
    for action in written {
        if !registered.contains(action) {
            missing.push(action);
        }
    }
    assert!(
        missing.is_empty(),
        "Rust writes unregistered audit actions: {missing:?}"
    );
}

#[test]
fn the_registry_parser_sees_the_canonical_set() {
    // Guard the guard: if the parser stops seeing the registry, the pin above
    // would pass vacuously. Python registers 51 actions today; every one the
    // parser finds must contain exactly one dot-namespace.
    let registered = registry_actions();
    assert!(
        registered.len() >= 40,
        "registry parse collapsed: {registered:?}"
    );
    for action in &registered {
        assert_eq!(action.match_indices('.').count(), 1, "{action}");
    }
    assert!(registered.contains("user.registered"));
    assert!(registered.contains("trade.paid"));
}
