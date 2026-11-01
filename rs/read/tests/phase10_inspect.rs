//! Phase 10 Task 23: read-only inspection tools, cases that need no database.
//!
//! The PostgreSQL cases live in `phase10_inspect_postgres.rs`. Here the pure
//! parts are pinned: masking, report shape, exit mapping, the registries the
//! blocker predicates use, and the source rules that keep the module read-only.
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use uuid::Uuid;
use verdaxis_read::analytics::usage::DEMO_MARKET_ORGANIZATION_IDS;
use verdaxis_read::cli::ops::inspect::{
    BlockerCounts, BlockersReport, IneligibleUser, KNOWN_TEST_ORGANIZATION_IDS, UsersReport,
    execute, mask_email,
};
use verdaxis_read::cli::{Environment, Exit, IdentityArgs, InspectArgs, InspectKind};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn id(number: u128) -> Uuid {
    Uuid::from_u128(number)
}

fn arguments(kind: InspectKind, json: bool, show_emails: bool) -> InspectArgs {
    InspectArgs {
        kind,
        identity: IdentityArgs {
            environment: Environment::Staging,
            release_sha: SHA.to_owned(),
        },
        json,
        show_emails,
    }
}

fn two_users() -> Vec<(Uuid, String, Option<String>)> {
    vec![
        (
            id(1),
            "alice.buyer@example.test".into(),
            Some("BUYER".into()),
        ),
        (id(2), "bob@example.test".into(), None),
    ]
}

// --- Masking ----------------------------------------------------------

#[test]
fn mask_keeps_one_character_and_the_domain() {
    assert_eq!(mask_email("alice.buyer@example.test"), "a***@example.test");
    assert_eq!(mask_email("b@example.test"), "b***@example.test");
    // The whole local part is hidden, whatever its length.
    assert!(!mask_email("a-very-long-local-part@example.test").contains("very"));
}

#[test]
fn mask_uses_the_last_at_sign_and_hides_a_quoted_tail() {
    assert_eq!(
        mask_email("\"secret@part\"@example.test"),
        "\"***@example.test"
    );
}

#[test]
fn mask_handles_values_that_are_not_addresses() {
    assert_eq!(mask_email("no-at-sign"), "***");
    assert_eq!(mask_email(""), "***");
    assert_eq!(mask_email("@example.test"), "***@example.test");
    // A leading space or control character never reaches the terminal.
    assert_eq!(mask_email(" dup@example.test"), "?***@example.test");
    assert_eq!(
        mask_email("\u{1b}[31mred@example.test"),
        "?***@example.test"
    );
    // The first character is a whole scalar value, not half of a UTF-8 sequence.
    assert_eq!(mask_email("é@example.test"), "é***@example.test");
}

#[test]
fn users_report_masks_unless_full_addresses_are_requested() {
    let masked = UsersReport::new(two_users(), 2, false);
    assert_eq!(masked.emails, "masked");
    assert_eq!(masked.users[0].email, "a***@example.test");
    assert_eq!(masked.users[1].email, "b***@example.test");
    for rendered in [
        masked.render_text(),
        serde_json::to_string(&masked).expect("serialize"),
    ] {
        assert!(!rendered.contains("alice.buyer"), "{rendered}");
        assert!(!rendered.contains("bob@"), "{rendered}");
    }

    let full = UsersReport::new(two_users(), 2, true);
    assert_eq!(full.emails, "full");
    assert_eq!(full.users[0].email, "alice.buyer@example.test");
    assert!(full.render_text().contains("alice.buyer@example.test"));
}

#[test]
fn users_text_marks_a_cut_listing_and_prints_none_for_a_missing_role() {
    let report = UsersReport::new(two_users(), 5, false);
    assert!(report.truncated);
    assert_eq!(report.listed, 2);
    let text = report.render_text();
    assert!(text.starts_with("Found 5 users:\n"), "{text}");
    assert!(text.contains("(Role: None)"), "{text}");
    assert!(
        text.contains("listing_truncated=true listed=2 total=5"),
        "{text}"
    );
    assert!(!UsersReport::new(two_users(), 2, false).truncated);
}

#[test]
fn full_addresses_cannot_carry_terminal_escapes() {
    let rows = vec![(id(3), "a\u{1b}[2Jb@example.test".to_owned(), None)];
    let text = UsersReport::new(rows, 1, true).render_text();
    assert!(!text.contains('\u{1b}'), "{text:?}");
}

#[tokio::test]
async fn show_emails_is_refused_for_blockers_before_any_read() {
    // The pool is lazy and points nowhere: a refusal must not need it.
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://nobody:none@127.0.0.1:1/none")
        .expect("lazy pool");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = execute(
        &pool,
        &arguments(InspectKind::Blockers, false, true),
        &mut stdout,
        &mut stderr,
    )
    .await;
    assert_eq!(exit, Exit::Usage);
    assert!(stdout.is_empty());
}

#[tokio::test]
async fn an_unreachable_database_prints_no_report_and_exits_4() {
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(300))
        .connect_lazy("postgres://nobody:none@127.0.0.1:1/none")
        .expect("lazy pool");
    for kind in [InspectKind::Users, InspectKind::Blockers] {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = execute(
            &pool,
            &arguments(kind, false, false),
            &mut stdout,
            &mut stderr,
        )
        .await;
        assert_eq!(exit, Exit::Dependency);
        assert!(stdout.is_empty(), "no partial report");
        let summary: Value =
            serde_json::from_slice(&stderr).expect("one JSON summary line on stderr");
        assert_eq!(summary["outcome"], "dependency_unavailable");
        // The summary names no host, role or password.
        let line = String::from_utf8(stderr).expect("utf8");
        assert!(!line.contains("nobody"), "{line}");
        assert!(!line.contains("postgres://"), "{line}");
    }
}

// --- Exit mapping and report shape ----------------------------------------

type Setter = fn(&mut BlockerCounts);

fn zero_counts() -> BlockerCounts {
    BlockerCounts::default()
}

#[test]
fn inspect_blockers_exits_2_when_any_count_is_nonzero_per_count() {
    let clean = BlockersReport::new(zero_counts(), vec![]);
    assert_eq!(clean.exit(), Exit::Ok);
    assert_eq!(clean.enforcement_preflight, "READY");

    // Each of the seven blocking counts alone must block.
    let setters: [(&str, Setter); 7] = [
        ("ineligible_users", |c| c.ineligible_users = 1),
        ("duplicate_case_insensitive_emails", |c| {
            c.duplicate_case_insensitive_emails = 1
        }),
        ("outstanding_orders", |c| {
            c.outstanding_legacy_orders_without_owner = 1
        }),
        ("outstanding_rfqs", |c| {
            c.outstanding_legacy_rfqs_without_owner = 1
        }),
        ("outstanding_quotes", |c| {
            c.outstanding_legacy_quotes_without_owner = 1
        }),
        ("outstanding_trades", |c| {
            c.outstanding_legacy_trades_without_user_provenance = 1
        }),
        ("outstanding_negotiations", |c| {
            c.outstanding_legacy_negotiations_without_user_provenance = 1
        }),
    ];
    for (name, set) in setters {
        let mut counts = zero_counts();
        set(&mut counts);
        let report = BlockersReport::new(counts, vec![]);
        assert_eq!(report.exit(), Exit::Usage, "{name} must block");
        assert_eq!(report.enforcement_preflight, "BLOCKED", "{name}");
        assert!(
            report
                .render_text()
                .contains("enforcement_preflight=BLOCKED\n"),
            "{name}"
        );
    }
    assert_eq!(Exit::Usage as i32, 2);
}

#[test]
fn counts_reported_for_visibility_never_block() {
    let counts = BlockerCounts {
        legacy_orders_without_owner: 9,
        legacy_rfqs_without_owner: 9,
        legacy_quotes_without_owner: 9,
        legacy_trades_without_user_provenance: 9,
        legacy_negotiations_without_user_provenance: 9,
        legacy_negotiation_rounds_without_user_provenance: 9,
        approved_kyc_without_external_review_evidence: 9,
        pending_organization_join_reviews: 9,
        ..BlockerCounts::default()
    };
    let report = BlockersReport::new(counts, vec![]);
    assert_eq!(report.exit(), Exit::Ok);
    assert_eq!(report.enforcement_preflight, "READY");
}

fn sample_blockers() -> BlockersReport {
    let counts = BlockerCounts {
        ineligible_users: 1,
        ..BlockerCounts::default()
    };
    let user = IneligibleUser {
        id: id(7),
        role: Some("BUYER".into()),
        status: "APPROVED".into(),
        email_verified: false,
        organization_id: None,
        kyc_status: "PENDING".into(),
    };
    BlockersReport::new(counts, vec![user])
}

#[test]
fn inspect_json_is_one_object_with_stable_keys() {
    let blockers: Value =
        serde_json::from_str(&serde_json::to_string(&sample_blockers()).unwrap()).unwrap();
    let keys: Vec<&str> = blockers
        .as_object()
        .expect("one object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "report",
            "enforcement_preflight",
            "kyc_enforcement",
            "legacy_owner_policy",
            "counts",
            "ineligible_users",
            "ineligible_users_truncated"
        ]
    );
    let count_keys: Vec<&str> = blockers["counts"]
        .as_object()
        .expect("counts object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        count_keys,
        [
            "ineligible_users",
            "duplicate_case_insensitive_emails",
            "legacy_orders_without_owner",
            "legacy_rfqs_without_owner",
            "legacy_quotes_without_owner",
            "legacy_trades_without_user_provenance",
            "legacy_negotiations_without_user_provenance",
            "legacy_negotiation_rounds_without_user_provenance",
            "approved_kyc_without_external_review_evidence",
            "pending_organization_join_reviews",
            "outstanding_legacy_orders_without_owner",
            "outstanding_legacy_rfqs_without_owner",
            "outstanding_legacy_quotes_without_owner",
            "outstanding_legacy_trades_without_user_provenance",
            "outstanding_legacy_negotiations_without_user_provenance",
        ]
    );
    let listed: Vec<&str> = blockers["ineligible_users"][0]
        .as_object()
        .expect("user object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        listed,
        [
            "id",
            "role",
            "status",
            "email_verified",
            "organization_id",
            "kyc_status"
        ]
    );
    // Exact empty values: an absent organization is null, not an empty string.
    assert_eq!(
        blockers["ineligible_users"][0]["organization_id"],
        Value::Null
    );
    assert_eq!(blockers["kyc_enforcement"], "ADVISORY_HELD");

    let users: Value = serde_json::from_str(
        &serde_json::to_string(&UsersReport::new(two_users(), 2, false)).unwrap(),
    )
    .unwrap();
    let keys: Vec<&str> = users
        .as_object()
        .expect("one object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["report", "total", "listed", "truncated", "emails", "users"]
    );
    let user_keys: Vec<&str> = users["users"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(user_keys, ["id", "email", "role"]);
    assert_eq!(users["users"][1]["role"], Value::Null);
}

#[test]
fn blockers_text_keeps_the_python_line_names_and_order() {
    let text = sample_blockers().render_text();
    let names: Vec<&str> = text
        .lines()
        .map(|line| line.split('=').next().expect("name"))
        .collect();
    assert_eq!(
        names,
        [
            "ineligible_users",
            "duplicate_case_insensitive_emails",
            "user",
            "legacy_orders_without_owner",
            "legacy_rfqs_without_owner",
            "legacy_quotes_without_owner",
            "legacy_trades_without_user_provenance",
            "legacy_negotiations_without_user_provenance",
            "legacy_negotiation_rounds_without_user_provenance",
            "approved_kyc_without_external_review_evidence",
            "kyc_enforcement",
            "pending_organization_join_reviews",
            "outstanding_legacy_orders_without_owner",
            "outstanding_legacy_rfqs_without_owner",
            "outstanding_legacy_quotes_without_owner",
            "outstanding_legacy_trades_without_user_provenance",
            "outstanding_legacy_negotiations_without_user_provenance",
            "enforcement_preflight",
            "legacy_owner_policy",
        ]
    );
    assert!(text.contains(
        "user=00000000-0000-0000-0000-000000000007 role=BUYER status=APPROVED \
         email_verified=False organization_id=None kyc_status=PENDING\n"
    ));
}

#[test]
fn a_cut_ineligible_listing_is_flagged_and_the_count_stays_exact() {
    let counts = BlockerCounts {
        ineligible_users: 500,
        ..BlockerCounts::default()
    };
    let report = BlockersReport::new(counts, vec![]);
    assert!(report.ineligible_users_truncated);
    assert_eq!(report.counts.ineligible_users, 500);
    assert!(
        report
            .render_text()
            .contains("ineligible_users_listing_truncated=0\n")
    );
}

// --- Registries -------------------------------------------------------------

/// The registries as the Python module defines them, read from its source so a
/// copy here cannot drift. Only `uuid` is imported by that module.
fn python_registry(name: &str) -> Vec<Uuid> {
    let interpreter = std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/demo_identities.py");
    let script = "import importlib.util, sys\n\
        spec = importlib.util.spec_from_file_location('demo_identities', sys.argv[1])\n\
        module = importlib.util.module_from_spec(spec)\n\
        spec.loader.exec_module(module)\n\
        print('\\n'.join(sorted(str(item) for item in getattr(module, sys.argv[2]))))\n";
    let output = std::process::Command::new(interpreter)
        .args(["-I", "-c", script])
        .arg(&source)
        .arg(name)
        .output()
        .expect("start the Python interpreter");
    assert!(output.status.success(), "reading {name} failed");
    String::from_utf8(output.stdout)
        .expect("utf8")
        .lines()
        .map(|line| Uuid::parse_str(line).expect("registry uuid"))
        .collect()
}

fn sorted(mut values: Vec<Uuid>) -> Vec<Uuid> {
    values.sort();
    values
}

#[test]
fn registries_equal_the_python_registries() {
    assert_eq!(
        sorted(DEMO_MARKET_ORGANIZATION_IDS.to_vec()),
        python_registry("DEMO_MARKET_ORG_IDS")
    );
    assert_eq!(
        sorted(KNOWN_TEST_ORGANIZATION_IDS.to_vec()),
        python_registry("KNOWN_TEST_ORG_IDS")
    );
}

// --- The module stays read-only --------------------------------------------

/// Source lines of the module without comments.
fn code_lines() -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli/ops/inspect.rs");
    std::fs::read_to_string(path)
        .expect("read the module source")
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_module_names_no_write_statement_file_access_or_commit() {
    let code = code_lines().join("\n");
    let uppercase = code.to_uppercase();
    // Assembled from parts so this file carries no write keyword itself.
    let write_words = [
        ["INSERT", " INTO"].concat(),
        ["UPDATE", " "].concat(),
        ["DELETE", " FROM"].concat(),
        ["TRUNCATE", " "].concat(),
        ["ALTER", " "].concat(),
        ["CREATE", " "].concat(),
        ["DROP", " "].concat(),
        ["GRANT", " "].concat(),
        ["COPY", " "].concat(),
        ["pg_advisory", ""].concat().to_uppercase(),
    ];
    for word in write_words {
        assert!(
            !uppercase.contains(&word),
            "the module must not contain {word:?}"
        );
    }
    for forbidden in [
        "std::fs",
        "File::",
        "OpenOptions",
        "tokio::fs",
        ".commit(",
        ".begin()",
    ] {
        assert!(
            !code.contains(forbidden),
            "the module must not contain {forbidden:?}"
        );
    }
    // One transaction opener, and it is the read-only one.
    assert_eq!(code.matches("begin_with(").count(), 1);
    assert!(code.contains("READ ONLY"));
}
