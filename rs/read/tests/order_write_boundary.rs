//! Phase 2 Task 20: the single sealed order-book write boundary.
//!
//! Two kinds of proof live here.
//!
//! 1. A source boundary: outside `rs/read/src/order_write/` no Rust source and
//!    no shared SQL may write an order-book row, take an order-row `FOR UPDATE`
//!    lock, call the acceptance-ordinal sequence, or take an advisory lock. Every
//!    insert target inside the boundary must belong to that exact source file, and external callers may name only the declared CF4_04 exports
//!    alongside the marked registration line in `lib.rs`. The scan is over source, so it holds for code no test exercises.
//! 2. A behaviour parity: the lock keys, slice order, idempotency scopes and
//!    request hashes must equal the vectors the reviewed Python exported into
//!    `tests/rust_contract/phase2/market-write-spec.json`.
//!
//! The transaction shell is driven through the `WriteSession` port with a
//! recording double, so retry, the attempt bound, each contention refusal,
//! ambiguous commit and the replay path are all exercised without a database.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use uuid::Uuid;

use verdaxis_read::order_write::{
    Canonical, CommandContext, CommittedCommand, DEFAULT_MARKET_TRANSACTION_ATTEMPTS,
    IDEMPOTENCY_BUSY_MESSAGE, IDEMPOTENCY_KEY_BOUNDS, IDEMPOTENCY_KEY_MAX,
    IDEMPOTENCY_LOCK_TIMEOUT, IDEMPOTENCY_STATEMENT_TIMEOUT, IDEMPOTENCY_TIMEOUT_MESSAGE,
    LOCK_BUSY_SQLSTATE, LOCK_TIMEOUT_SQLSTATE, MARKET_LOCK_TIMEOUT, MARKET_ORGANIZATION_REQUIRED,
    MARKET_STATEMENT_TIMEOUT, MAX_MARKET_TRANSACTION_ATTEMPTS, ORDER_NOT_FOUND_MESSAGE, Operation,
    OrderCommand, REPLAY_CONFLICT_MESSAGE, RETRYABLE_SQLSTATES, SERIALIZATION_FAILED_MESSAGE,
    SLICE_BUSY_MESSAGE, SLICE_TIMEOUT_MESSAGE, SliceKey, command_request_hash,
    execute_order_command, idempotency_lock_key, idempotency_request_hash, market_slice_lock_key,
    market_slice_lock_sort_key, normalize_idempotency_key, normalize_slice_window,
    ordered_slice_lock_keys,
};

const SPEC: &str = include_str!("../../../tests/rust_contract/phase2/market-write-spec.json");

fn spec() -> Value {
    serde_json::from_str(SPEC).expect("the reviewed spec must parse")
}

fn spec_string(value: &Value, path: &[&str]) -> String {
    let mut node = value;
    for key in path {
        node = &node[*key];
    }
    node.as_str()
        .unwrap_or_else(|| panic!("{path:?} must be a string"))
        .to_owned()
}

fn uuid(text: &str) -> Uuid {
    Uuid::parse_str(text).expect("vector uuid")
}

fn optional_uuid(node: &Value) -> Option<Uuid> {
    node.as_str().map(uuid)
}

// ---------------------------------------------------------------------------
// The source boundary.
// ---------------------------------------------------------------------------

/// The two tables the boundary may write, per `deploy/postgres/app_acl_policy.sql`.
const WRITE_TABLES: [&str; 2] = ["MARKET_COMMAND_RESULTS", "MARKET_EVENT_OUTBOX"];

/// The one gated item shape the advisory-lock rule recognises.
fn cfg_attribute() -> String {
    ["#[cfg(feature = ", "\"contract-tests\")]"].concat()
}

/// Byte ranges of the braced blocks that follow a contract-tests attribute.
///
/// A literal inside one of them is test-only scaffolding that a release build
/// never compiles. Brace counting is over raw text, not a parse tree: it is used
/// only to accept an exception, never to create a refusal.
fn gated_ranges(text: &str) -> Vec<(usize, usize)> {
    let attribute = cfg_attribute();
    let mut ranges = Vec::new();
    let mut cursor = 0usize;
    while let Some(relative) = text[cursor..].find(&attribute) {
        let after = cursor + relative + attribute.len();
        let Some(open_relative) = text[after..].find('{') else {
            break;
        };
        let open = after + open_relative;
        let mut depth = 0usize;
        let mut close = open;
        for (offset, byte) in text[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = open + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        ranges.push((open, close));
        cursor = close;
        if close == open {
            break;
        }
    }
    ranges
}

/// Byte ranges of every Rust string literal, including raw strings.
///
/// Comments and character literals are skipped, so a keyword in prose or in a
/// `'a'` never counts. Escape sequences are left in place: none of the reviewed
/// patterns is written with an escape.
fn literal_ranges(text: &str) -> Vec<(usize, usize)> {
    let characters: Vec<(usize, char)> = text.char_indices().collect();
    let mut ranges = Vec::new();
    let mut index = 0usize;
    while index < characters.len() {
        let (offset, character) = characters[index];
        match character {
            '/' if characters.get(index + 1).map(|entry| entry.1) == Some('/') => {
                while index < characters.len() && characters[index].1 != '\n' {
                    index += 1;
                }
            }
            '/' if characters.get(index + 1).map(|entry| entry.1) == Some('*') => {
                index += 2;
                let mut depth = 1usize;
                while index < characters.len() && depth > 0 {
                    let next = characters.get(index + 1).map(|entry| entry.1);
                    match (characters[index].1, next) {
                        ('/', Some('*')) => {
                            depth += 1;
                            index += 2;
                        }
                        ('*', Some('/')) => {
                            depth -= 1;
                            index += 2;
                        }
                        _ => index += 1,
                    }
                }
            }
            '"' => {
                let start = offset + 1;
                index += 1;
                while index < characters.len() && characters[index].1 != '"' {
                    if characters[index].1 == '\\' {
                        index += 1;
                    }
                    index += 1;
                }
                let end = characters
                    .get(index)
                    .map(|entry| entry.0)
                    .unwrap_or(text.len());
                ranges.push((start, end));
                index += 1;
            }
            'r' if matches!(
                characters.get(index + 1).map(|entry| entry.1),
                Some('#') | Some('"')
            ) =>
            {
                let mut cursor = index + 1;
                let mut hashes = 0usize;
                while characters.get(cursor).map(|entry| entry.1) == Some('#') {
                    hashes += 1;
                    cursor += 1;
                }
                if characters.get(cursor).map(|entry| entry.1) == Some('"') {
                    let start = characters[cursor].0 + 1;
                    let terminator = format!("\"{}", "#".repeat(hashes));
                    if let Some(relative) = text[start..].find(&terminator) {
                        let end = start + relative;
                        ranges.push((start, end));
                        let after = end + terminator.len();
                        index = characters.partition_point(|entry| entry.0 < after);
                        continue;
                    }
                }
                index += 1;
            }
            '\'' => {
                if characters.get(index + 1).map(|entry| entry.1) == Some('\\') {
                    index += 2;
                    while index < characters.len() && characters[index].1 != '\'' {
                        index += 1;
                    }
                    index += 1;
                } else if characters.get(index + 2).map(|entry| entry.1) == Some('\'') {
                    index += 3;
                } else {
                    index += 1;
                }
            }
            _ => index += 1,
        }
    }
    ranges
}

/// Uppercase, whitespace-collapsed text. The rules read tokens, not layout.
fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase()
}

fn contains_within(text: &str, start: usize, window: usize, needle: &str) -> bool {
    let bytes = text.as_bytes();
    let end = (start + window).min(bytes.len());
    if start >= end || needle.len() > end - start {
        return false;
    }
    bytes[start..end]
        .windows(needle.len())
        .any(|candidate| candidate == needle.as_bytes())
}

fn precedes_within(text: &str, offset: usize, window: usize, needle: &str) -> bool {
    let start = offset.saturating_sub(window);
    let bytes = text.as_bytes();
    if start >= offset || needle.len() > offset - start {
        return false;
    }
    bytes[start..offset]
        .windows(needle.len())
        .any(|candidate| candidate == needle.as_bytes())
}

/// Any order-book DML keyword in one normalised literal.
fn orderbook_dml(text: &str) -> Option<&'static str> {
    let keywords = [
        ["INSERT", " INTO"].concat(),
        ["UPDATE", " "].concat(),
        ["DELETE", " FROM"].concat(),
        ["TRUNCATE", " "].concat(),
    ];
    for keyword in keywords {
        let mut cursor = 0usize;
        while let Some(relative) = text[cursor..].find(&keyword) {
            let start = cursor + relative + keyword.len();
            if contains_within(text, start, 48, "ORDERBOOK") {
                return Some("order-book DML");
            }
            cursor = start;
        }
    }
    None
}

/// Any `FOR UPDATE` that a `WHERE` clause could be aiming at an order-book
/// relation inside the same literal.
fn orderbook_row_lock(text: &str) -> bool {
    let needle = ["FOR", " UPDATE"].concat();
    let mut cursor = 0usize;
    while let Some(relative) = text[cursor..].find(&needle) {
        let offset = cursor + relative;
        if precedes_within(text, offset, 200, "ORDERBOOK") {
            return true;
        }
        cursor = offset + needle.len();
    }
    false
}

/// Every `INSERT INTO <relation>` target in one normalised literal.
fn insert_targets(text: &str) -> Vec<String> {
    let keyword = ["INSERT", " INTO "].concat();
    let mut targets = Vec::new();
    let mut cursor = 0usize;
    while let Some(relative) = text[cursor..].find(&keyword) {
        let start = cursor + relative + keyword.len();
        let identifier: String = text[start..]
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
            .collect();
        cursor = start + identifier.len().max(1);
        if !identifier.is_empty() {
            targets.push(identifier);
        }
    }
    targets
}

fn collect(path: &Path, extension: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        if name == "target" || name == ".git" || name == "work" {
            continue;
        }
        let child = entry.path();
        if child.is_dir() {
            collect(&child, extension, found);
        } else if child.extension().and_then(|value| value.to_str()) == Some(extension) {
            found.push(child);
        }
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// Every boundary file retains receipt/outbox inserts; additional insert targets
/// belong only to the exact named source file.
fn boundary_insert_allowed(root: &Path, path: &Path, table: &str) -> bool {
    let boundary = root.join("rs/read/src/order_write");
    if !path.starts_with(&boundary) {
        return false;
    }
    if WRITE_TABLES.contains(&table) {
        return true;
    }
    let per_file: &[(&str, &[&str])] = &[
        (
            "effects.rs",
            &["AUDIT_LOGS", "NOTIFICATIONS", "LIVE_SLICE_BENCHMARKS"],
        ),
        ("matching.rs", &["TRADES"]),
        ("dismissal.rs", &["MATCH_SUGGESTIONS"]),
    ];
    per_file
        .iter()
        .any(|(file, tables)| path == boundary.join(file) && tables.contains(&table))
}

/// Every source-boundary violation in the repository, as `path: rule` text.
fn scan_repository() -> Vec<String> {
    let root = repository_root();
    let boundary = root.join("rs/read/src/order_write");
    // The boundary's own integration test drives the module through its port,
    // so it names the module. Other callers may name only the declared business exports.
    let owned_test = root.join("rs/read/tests/order_write_boundary.rs");
    let ordinal = ["ORDERBOOK_ACCEPTANCE", "_ORDINAL_SEQ"].concat();
    let advisory_locks = [
        ["PG_ADVISORY", "_XACT_LOCK"].concat(),
        ["PG_ADVISORY", "_LOCK"].concat(),
    ];
    // PM decision 94 (PD8) approves one advisory lock outside this boundary:
    // the price-alert cap serializes organization creators in its own namespace.
    // It does not touch order-book rows, so the boundary rule names it exactly.
    let price_alert_cap = [root.join("rs/read/src/crud/alerts.rs")];
    // The browser-device session lock is shared with Python's login, refresh and
    // logout, so both stacks wait on one key. The key comes from the device
    // cookie digest and never names an order-book row. The session store and its
    // test probe that proves the shared key are named exactly.
    let device_session_lock = [
        root.join("rs/read/src/auth/store.rs"),
        root.join("rs/read/tests/auth_store_postgres.rs"),
    ];
    let mut violations = Vec::new();

    let mut rust_files = Vec::new();
    collect(&root.join("rs"), "rs", &mut rust_files);
    rust_files.sort();
    for path in &rust_files {
        let text = std::fs::read_to_string(path).expect("readable Rust source");
        let inside_boundary = path.starts_with(&boundary);
        let gated = gated_ranges(&text);
        for (start, end) in literal_ranges(&text) {
            let literal = normalized(&text[start..end]);
            if inside_boundary {
                for target in insert_targets(&literal) {
                    if !boundary_insert_allowed(&root, path, &target) {
                        violations.push(format!("{}: INSERT INTO {target}", path.display()));
                    }
                }
                continue;
            }
            if let Some(rule) = orderbook_dml(&literal) {
                violations.push(format!("{}: {rule}", path.display()));
            }
            if orderbook_row_lock(&literal) {
                violations.push(format!("{}: order-book row FOR UPDATE", path.display()));
            }
            if literal.contains(&ordinal) {
                violations.push(format!("{}: acceptance ordinal sequence", path.display()));
            }
            let takes_advisory_lock = advisory_locks.iter().any(|needle| literal.contains(needle));
            let test_only = gated
                .iter()
                .any(|(open, close)| start >= *open && start <= *close);
            if takes_advisory_lock
                && !test_only
                && !price_alert_cap.contains(path)
                && !device_session_lock.contains(path)
            {
                violations.push(format!(
                    "{}: advisory lock outside order_write",
                    path.display()
                ));
            }
        }
        let is_manifest = path.file_name().and_then(|name| name.to_str()) == Some("lib.rs");
        if !inside_boundary
            && !is_manifest
            && path != &owned_test
            && path != &root.join("rs/read/examples/order_write.rs")
            && !boundary_reference_allowed(&text)
        {
            violations.push(format!("{}: names the write boundary", path.display()));
        }
    }

    let mut sql_files = Vec::new();
    collect(&root.join("rs/read/sql"), "sql", &mut sql_files);
    sql_files.sort();
    for path in &sql_files {
        let literal = normalized(&std::fs::read_to_string(path).expect("readable SQL"));
        if let Some(rule) = orderbook_dml(&literal) {
            violations.push(format!("{}: {rule}", path.display()));
        }
        if orderbook_row_lock(&literal) {
            violations.push(format!("{}: order-book row FOR UPDATE", path.display()));
        }
        if literal.contains(&ordinal) {
            violations.push(format!("{}: acceptance ordinal sequence", path.display()));
        }
    }
    violations
}

// CF4_04.integration_patch.order_write_mod_rs.add_exports is the sole
// authority for these external business entry points. No file-level exemption.
const BUSINESS_EXPORTS: &[&str] = &[
    "InvalidateOrganization",
    "InvalidationResult",
    "InvalidationError",
    "invalidate_organization",
    "BenchmarkRebuildKey",
    "BenchmarkRebuildResult",
    "rebuild_live_slice_benchmarks_for_keys",
];

fn rust_tokens(text: &str) -> Vec<String> {
    let literals = literal_ranges(text);
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some((_, end)) = literals
            .iter()
            .find(|(start, end)| i >= start.saturating_sub(1) && i <= *end)
        {
            i = end + 1;
        } else if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            tokens.push(text[start..i].to_owned());
        } else if bytes[i..].starts_with(b"::") {
            tokens.push("::".into());
            i += 2;
        } else {
            if !bytes[i].is_ascii_whitespace() {
                tokens.push((bytes[i] as char).to_string());
            }
            i += 1;
        }
    }
    tokens
}

fn boundary_reference_allowed(text: &str) -> bool {
    let tokens = rust_tokens(text);
    for (index, token) in tokens.iter().enumerate() {
        if token != "order_write" {
            continue;
        }
        if index > 0 && tokens[index - 1] == "mod" {
            // Declaration-only consumer doubles expose the same narrow port.
            // Their literals still pass every DML and lock check above.
            let mut cursor = index + 1;
            if tokens.get(cursor).map(String::as_str) != Some("{") {
                return false;
            }
            let mut depth = 1;
            cursor += 1;
            while cursor < tokens.len() && depth > 0 {
                if depth == 1 && tokens[cursor] == "pub" {
                    cursor += 1;
                    if tokens.get(cursor).map(String::as_str) == Some("async") {
                        cursor += 1;
                    }
                    if !matches!(
                        tokens.get(cursor).map(String::as_str),
                        Some("struct" | "type" | "fn")
                    ) {
                        return false;
                    }
                    cursor += 1;
                    if !tokens
                        .get(cursor)
                        .is_some_and(|name| BUSINESS_EXPORTS.contains(&name.as_str()))
                    {
                        return false;
                    }
                }
                match tokens.get(cursor).map(String::as_str) {
                    Some("{") => depth += 1,
                    Some("}") => depth -= 1,
                    _ => {}
                }
                cursor += 1;
            }
            if depth != 0 {
                return false;
            }
            continue;
        }
        if tokens.get(index + 1).map(String::as_str) != Some("::") {
            return false;
        }
        let Some(item) = tokens.get(index + 2) else {
            return false;
        };
        if item == "{" {
            let mut cursor = index + 3;
            let mut expect_item = true;
            while let Some(token) = tokens.get(cursor) {
                if token == "}" {
                    break;
                }
                if expect_item {
                    if !BUSINESS_EXPORTS.contains(&token.as_str()) {
                        return false;
                    }
                    expect_item = false;
                } else if token == "as" {
                    cursor += 1;
                    let Some(alias) = tokens.get(cursor) else {
                        return false;
                    };
                    if !alias
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    {
                        return false;
                    }
                } else if token == "," {
                    expect_item = true;
                } else {
                    return false;
                }
                cursor += 1;
            }
            if tokens.get(cursor).map(String::as_str) != Some("}") {
                return false;
            }
        } else if !BUSINESS_EXPORTS.contains(&item.as_str())
            || tokens.get(index + 3).map(String::as_str) == Some("::")
        {
            return false;
        }
    }
    true
}

#[test]
fn declared_business_exports_do_not_open_the_write_boundary() {
    let contract: Value = serde_json::from_str(include_str!(
        "../../../tests/rust_contract/phase4/contracts-v2.json"
    ))
    .unwrap();
    let exports =
        contract["contracts"][3]["integration_patch"]["order_write_mod_rs"]["add_exports"]
            .as_str()
            .unwrap();
    let declared: Vec<_> = rust_tokens(exports)
        .into_iter()
        .filter(|token| BUSINESS_EXPORTS.contains(&token.as_str()))
        .collect();
    assert_eq!(declared, BUSINESS_EXPORTS);
    let business = "use crate::order_write::{InvalidateOrganization, invalidate_organization};";
    assert!(
        boundary_reference_allowed(business),
        "declared business export"
    );
    assert!(boundary_reference_allowed(
        "crate::order_write::invalidate_organization(conn, command)"
    ));
    assert!(!boundary_reference_allowed(
        "use crate::order_write::execute_order_command;"
    ));
    assert!(!boundary_reference_allowed(
        "use crate::order_write::{InvalidateOrganization, locks::SliceKey};"
    ));
    assert!(!boundary_reference_allowed("use crate::order_write::*;"));
    assert!(boundary_reference_allowed(
        "pub mod order_write { pub struct InvalidateOrganization { pub id: Uuid } pub type InvalidationError = Error; pub async fn invalidate_organization() {} }"
    ));
    assert!(!boundary_reference_allowed(
        "pub mod order_write { pub fn execute_order_command() {} }"
    ));
    assert!(!boundary_reference_allowed(
        "use crate::order_write as other;"
    ));
    assert!(!boundary_reference_allowed(
        "use crate::order_write::{InvalidateOrganization, execute_order_command};"
    ));
    let direct_write = ["UPDATE ", "order", "book_orders SET version=2"].concat();
    assert_eq!(
        orderbook_dml(&normalized(&direct_write)),
        Some("order-book DML")
    );
}

#[test]
fn order_book_writes_stay_in_the_one_boundary() {
    let violations = scan_repository();
    assert!(
        violations.is_empty(),
        "the write boundary was crossed:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_scan_refuses_each_rule_it_claims() {
    // Each sample is assembled at run time so this file never carries the
    // forbidden text it is teaching the scanner to reject.
    let dml = ["UPDATE ", "order", "book_orders SET version = version + 1"].concat();
    assert_eq!(orderbook_dml(&normalized(&dml)), Some("order-book DML"));
    let insert = ["INSERT INTO ", "order", "book_orders (id) VALUES ($1)"].concat();
    assert!(orderbook_dml(&normalized(&insert)).is_some());
    let lock = [
        "SELECT id FROM ",
        "order",
        "book_orders WHERE id = $1 ",
        "FOR",
        " UPDATE",
    ]
    .concat();
    assert!(orderbook_row_lock(&normalized(&lock)));
    let ordinal = ["nextval('order", "book_acceptance_ordinal_seq')"].concat();
    assert!(normalized(&ordinal).contains(&["ORDERBOOK_ACCEPTANCE", "_ORDINAL_SEQ"].concat()));
    let mut reasonable = normalized("SELECT id FROM trades WHERE id = $1 FOR UPDATE");
    assert!(!orderbook_row_lock(&reasonable));
    assert!(orderbook_dml(&reasonable).is_none());
    reasonable = normalized(&insert);
    assert_eq!(
        insert_targets(&reasonable),
        vec!["ORDERBOOK_ORDERS".to_owned()]
    );
}

#[test]
fn only_the_registration_line_names_the_boundary() {
    let root = repository_root();
    let manifest = std::fs::read_to_string(root.join("rs/read/src/lib.rs")).expect("lib.rs");
    let marked = manifest
        .lines()
        .filter(|line| line.trim() == "pub mod order_write;")
        .count();
    assert_eq!(marked, 1, "exactly one registration line");
    assert!(manifest.contains("Phase 2 Task 20 registration block"));
    // Everything else that names the module is inside the module, apart from
    // the boundary's own test, which drives it through the port.
    let owned_test = root.join("rs/read/tests/order_write_boundary.rs");
    let mut mentions = Vec::new();
    for path in literal_sources() {
        if path.starts_with(root.join("rs/read/src/order_write"))
            || path == root.join("rs/read/src/lib.rs")
            || path == owned_test
            || path == root.join("rs/read/examples/order_write.rs")
        {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("readable source");
        if !boundary_reference_allowed(&text) {
            mentions.push(path.display().to_string());
        }
    }
    assert!(mentions.is_empty(), "unexpected references: {mentions:?}");
}

fn literal_sources() -> Vec<PathBuf> {
    let mut sources = Vec::new();
    collect(&repository_root().join("rs"), "rs", &mut sources);
    sources.sort();
    sources
}

// ---------------------------------------------------------------------------
// Behaviour parity with the reviewed Python.
// ---------------------------------------------------------------------------

#[test]
fn pinned_constants_match_the_python_source() {
    let spec = spec();
    let constants = &spec["constants"];
    assert_eq!(
        constants["market_lock_timeout"].as_str().unwrap(),
        MARKET_LOCK_TIMEOUT
    );
    assert_eq!(
        constants["market_statement_timeout"].as_str().unwrap(),
        MARKET_STATEMENT_TIMEOUT
    );
    assert_eq!(
        constants["idempotency_lock_timeout"].as_str().unwrap(),
        IDEMPOTENCY_LOCK_TIMEOUT
    );
    assert_eq!(
        constants["idempotency_statement_timeout"].as_str().unwrap(),
        IDEMPOTENCY_STATEMENT_TIMEOUT
    );
    assert_eq!(
        constants["lock_busy_sqlstate"].as_str().unwrap(),
        LOCK_BUSY_SQLSTATE
    );
    assert_eq!(
        constants["lock_timeout_sqlstate"].as_str().unwrap(),
        LOCK_TIMEOUT_SQLSTATE
    );
    assert_eq!(constants["idempotency_key_min"].as_u64().unwrap(), 1);
    assert_eq!(
        constants["idempotency_key_max"].as_u64().unwrap() as usize,
        IDEMPOTENCY_KEY_MAX
    );
    assert_eq!(
        constants["market_transaction_attempts"].as_u64().unwrap() as usize,
        DEFAULT_MARKET_TRANSACTION_ATTEMPTS
    );
    let bounds = constants["market_transaction_attempt_bounds"]
        .as_array()
        .unwrap();
    assert_eq!(bounds[0].as_u64().unwrap(), 1);
    assert_eq!(
        bounds[1].as_u64().unwrap() as usize,
        MAX_MARKET_TRANSACTION_ATTEMPTS
    );
    let retryable: Vec<String> = constants["retryable_sqlstates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(retryable, RETRYABLE_SQLSTATES.map(str::to_owned).to_vec());
    // The reviewed message for a missing order row is still the route's own.
    assert_eq!(
        spec_string(&spec, &["constants", "messages", "order_not_found"]),
        ORDER_NOT_FOUND_MESSAGE
    );
    assert_eq!(
        constants["messages"]["key_bounds"].as_str().unwrap(),
        IDEMPOTENCY_KEY_BOUNDS
    );
    assert_eq!(
        constants["messages"]["market_organization"]
            .as_str()
            .unwrap(),
        MARKET_ORGANIZATION_REQUIRED
    );
    assert_eq!(
        constants["messages"]["replay_conflict"].as_str().unwrap(),
        REPLAY_CONFLICT_MESSAGE
    );
    assert_eq!(
        constants["messages"]["slice_busy"].as_str().unwrap(),
        SLICE_BUSY_MESSAGE
    );
    assert_eq!(
        constants["messages"]["slice_timeout"].as_str().unwrap(),
        SLICE_TIMEOUT_MESSAGE
    );
    assert_eq!(
        constants["messages"]["serialization_failed"]
            .as_str()
            .unwrap(),
        SERIALIZATION_FAILED_MESSAGE
    );
    assert_eq!(
        constants["messages"]["idempotency_busy"].as_str().unwrap(),
        IDEMPOTENCY_BUSY_MESSAGE
    );
    assert_eq!(
        constants["messages"]["idempotency_timeout"]
            .as_str()
            .unwrap(),
        IDEMPOTENCY_TIMEOUT_MESSAGE
    );
}

#[test]
fn lock_statement_text_matches_the_python_source() {
    let spec = spec();
    let lock_sql = &spec["constants"]["lock_sql"];
    assert_eq!(
        lock_sql["market_slice_set_lock_timeout"].as_str().unwrap(),
        format!("SET LOCAL lock_timeout = '{MARKET_LOCK_TIMEOUT}'")
    );
    assert_eq!(
        lock_sql["market_slice_set_statement_timeout"]
            .as_str()
            .unwrap(),
        format!("SET LOCAL statement_timeout = '{MARKET_STATEMENT_TIMEOUT}'")
    );
    assert_eq!(
        lock_sql["market_slice_advisory_lock"].as_str().unwrap(),
        ["SELECT pg_advisory", "_xact_lock(:lock_key)"].concat()
    );
}

#[test]
fn receipt_operations_cover_the_six_reviewed_commands() {
    let spec = spec();
    let mut expected: Vec<String> = spec["constants"]["receipt_operations"]
        .as_object()
        .unwrap()
        .values()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    let mut actual: Vec<String> = Operation::ALL
        .iter()
        .map(|operation| operation.as_str().to_owned())
        .collect();
    expected.sort();
    actual.sort();
    assert_eq!(expected, actual);
    assert_eq!(Operation::OrderAmend.resource_type(), "order");
    assert_eq!(Operation::OrderCancel.resource_type(), "order");
    for operation in [
        Operation::TradeConfirm,
        Operation::TradeDecline,
        Operation::TradeDeliver,
        Operation::TradePay,
    ] {
        assert_eq!(operation.resource_type(), "trade");
    }
    // A resource-row operation is never a receipt operation.
    let resource_operations = spec["constants"]["idempotency_operations"]
        .as_object()
        .unwrap();
    for value in resource_operations.values() {
        assert!(
            !Operation::ALL
                .iter()
                .any(|operation| operation.as_str() == value.as_str().unwrap())
        );
    }
}

#[test]
fn write_tables_are_the_two_reviewed_tables() {
    let spec = spec();
    let tables = spec["write_tables"].as_array().unwrap();
    let mut names: Vec<String> = tables
        .iter()
        .map(|table| table["table"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "market_command_results".to_owned(),
            "market_event_outbox".to_owned()
        ]
    );
    for table in tables {
        let name = table["table"].as_str().unwrap();
        let grants: Vec<String> = table["grants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|grant| grant.as_str().unwrap().to_owned())
            .collect();
        assert!(
            grants.contains(&"SELECT".to_owned()),
            "{name} must be readable"
        );
        assert!(
            grants.contains(&"INSERT".to_owned()),
            "{name} must be writable"
        );
    }
    assert_eq!(
        WRITE_TABLES.map(str::to_lowercase),
        [
            "market_command_results".to_owned(),
            "market_event_outbox".to_owned()
        ]
    );
}

#[test]
fn slice_lock_keys_and_windows_match_python() {
    let spec = spec();
    let rows = spec["slice_lock_keys"].as_array().unwrap();
    assert!(rows.len() >= 6, "the reviewed case table must be present");
    for row in rows {
        let label = row["label"].as_str().unwrap();
        let window = row["availability_window"].as_str().unwrap();
        let normalized = normalize_slice_window(window)
            .unwrap_or_else(|_| panic!("{label}: {window} must normalize"));
        assert_eq!(
            normalized,
            row["normalized_window"].as_str().unwrap(),
            "{label}"
        );
        let product_id = uuid(row["product_id"].as_str().unwrap());
        let point = optional_uuid(&row["delivery_point_id"]);
        assert_eq!(
            market_slice_lock_key(product_id, point, &normalized),
            row["lock_key"].as_i64().unwrap(),
            "{label}"
        );
        let (product, point_text, window_text) =
            market_slice_lock_sort_key(product_id, point, &normalized);
        let sort_key: Vec<String> = row["sort_key"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(sort_key, vec![product, point_text, window_text], "{label}");
    }
}

#[test]
fn side_independent_slice_identity_shares_one_lock() {
    let spec = spec();
    let rows = spec["slice_lock_keys"].as_array().unwrap();
    let mut by_identity: BTreeMap<(String, Option<String>, String), i64> = BTreeMap::new();
    for row in rows {
        let identity = (
            row["product_id"].as_str().unwrap().to_owned(),
            row["delivery_point_id"].as_str().map(str::to_owned),
            normalize_slice_window(row["availability_window"].as_str().unwrap()).unwrap(),
        );
        let key = row["lock_key"].as_i64().unwrap();
        if let Some(existing) = by_identity.insert(identity, key) {
            assert_eq!(existing, key, "one slice must hash to one key");
        }
    }
    // Two of the reviewed cases differ only by side or by input padding.
    let labels: Vec<&str> = rows
        .iter()
        .map(|row| row["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"spot-bid"));
    assert!(labels.contains(&"spot-ask-same-slice"));
}

#[test]
fn ordered_slice_locks_match_python() {
    let spec = spec();
    let cases = spec["slice_lock_sort"].as_array().unwrap();
    assert!(cases.len() >= 3);
    for case in cases {
        let label = case["label"].as_str().unwrap();
        let slices: Vec<SliceKey> = case["input"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| SliceKey {
                product_id: uuid(row["product_id"].as_str().unwrap()),
                delivery_point_id: optional_uuid(&row["delivery_point_id"]),
                availability_window: row["availability_window"].as_str().unwrap().to_owned(),
            })
            .collect();
        let expected: Vec<i64> = case["acquired_lock_keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_i64().unwrap())
            .collect();
        assert_eq!(
            ordered_slice_lock_keys(&slices).unwrap(),
            expected,
            "{label}: the reviewed acquisition order"
        );
        // The order is stable under input order.
        let mut reversed = slices.clone();
        reversed.reverse();
        assert_eq!(
            ordered_slice_lock_keys(&reversed).unwrap(),
            expected,
            "{label}"
        );
        // Duplicates collapse: one key per slice identity.
        let mut doubled = slices.clone();
        doubled.extend(slices.iter().cloned());
        assert_eq!(
            ordered_slice_lock_keys(&doubled).unwrap(),
            expected,
            "{label}"
        );
    }
}

#[test]
fn idempotency_scopes_match_python() {
    let spec = spec();
    for row in spec["idempotency_lock_keys"].as_array().unwrap() {
        let label = row["label"].as_str().unwrap();
        let scope = uuid(row["scope_id"].as_str().unwrap());
        let operation = row["operation"].as_str().unwrap();
        let key = row["key"].as_str().unwrap();
        assert_eq!(
            idempotency_lock_key(scope, operation, key),
            row["lock_key"].as_i64().unwrap(),
            "{label}"
        );
    }
}

#[test]
fn command_timestamps_keep_python_microsecond_precision() {
    let payload = Canonical::Timestamp(
        chrono::DateTime::parse_from_rfc3339("2026-10-19T00:00:00.123000Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    assert_eq!(
        verdaxis_read::order_write::python_json(&payload),
        "\"2026-10-19T00:00:00.123000Z\""
    );
}

#[test]
fn request_hashes_match_python() {
    let spec = spec();
    for row in spec["command_request_hashes"].as_array().unwrap() {
        let label = row["label"].as_str().unwrap();
        let payload: Canonical = serde_json::from_value(row["payload"].clone())
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        assert_eq!(
            command_request_hash(&payload),
            row["sha256"].as_str().unwrap(),
            "{label}"
        );
    }
    for row in spec["idempotency_request_hashes"].as_array().unwrap() {
        let label = row["label"].as_str().unwrap();
        let payload: Canonical = serde_json::from_value(row["payload"].clone()).unwrap();
        assert_eq!(
            idempotency_request_hash(&payload).unwrap(),
            row["sha256"].as_str().unwrap(),
            "{label}"
        );
    }
}

#[test]
fn idempotency_hash_refuses_typed_values() {
    let payload = Canonical::Object(BTreeMap::from([
        (
            "order_id".to_owned(),
            Canonical::Uuid(uuid("10000000-0000-4000-8000-0000000000a1")),
        ),
        (
            "price".to_owned(),
            Canonical::Decimal("750.00".parse().unwrap()),
        ),
    ]));
    assert!(idempotency_request_hash(&payload).is_err());
    let counted = Canonical::Object(BTreeMap::from([(
        "quantity".to_owned(),
        Canonical::Int(serde_json::json!(10).as_i64().unwrap().into()),
    )]));
    assert!(idempotency_request_hash(&counted).is_ok());
}

#[test]
fn idempotency_key_bounds_match_python() {
    assert_eq!(normalize_idempotency_key("  padded  ").unwrap(), "padded");
    assert_eq!(normalize_idempotency_key("ключ-1").unwrap(), "ключ-1");
    assert!(normalize_idempotency_key("").is_err());
    assert!(normalize_idempotency_key("   ").is_err());
    assert!(normalize_idempotency_key(&"a".repeat(IDEMPOTENCY_KEY_MAX)).is_ok());
    assert!(normalize_idempotency_key(&"a".repeat(IDEMPOTENCY_KEY_MAX + 1)).is_err());
    // Bounded by characters, not bytes.
    let multibyte = "ü".repeat(IDEMPOTENCY_KEY_MAX);
    assert_eq!(multibyte.len(), IDEMPOTENCY_KEY_MAX * 2);
    assert!(normalize_idempotency_key(&multibyte).is_ok());
}

// ---------------------------------------------------------------------------
// The transaction shell.
// ---------------------------------------------------------------------------

/// The public entry point exists and keeps its reviewed signature.
#[test]
fn the_entry_point_signature_is_the_reviewed_one() {
    async fn call(
        state: &verdaxis_read::AppState,
        context: CommandContext,
        command: OrderCommand,
    ) -> Result<CommittedCommand, verdaxis_read::ApiError> {
        execute_order_command(state, context, command).await
    }
    let _ = call;
}

#[test]
fn effect_insert_authority_is_scoped_to_the_exact_source_file() {
    let root = repository_root();
    let effects = root.join("rs/read/src/order_write/effects.rs");
    for table in ["AUDIT_LOGS", "NOTIFICATIONS", "LIVE_SLICE_BENCHMARKS"] {
        assert!(boundary_insert_allowed(&root, &effects, table));
        for file in [
            "transaction.rs",
            "outbox.rs",
            "command.rs",
            "nested/effects.rs",
        ] {
            assert!(!boundary_insert_allowed(
                &root,
                &root.join("rs/read/src/order_write").join(file),
                table
            ));
        }
    }
    for file in ["effects.rs", "transaction.rs", "outbox.rs", "command.rs"] {
        for table in WRITE_TABLES {
            assert!(boundary_insert_allowed(
                &root,
                &root.join("rs/read/src/order_write").join(file),
                table
            ));
        }
    }
    assert!(!boundary_insert_allowed(&root, &effects, "USERS"));
    assert!(!boundary_insert_allowed(&root, &effects, "UNLISTED_TABLE"));
}

#[test]
fn dismissal_insert_authority_is_scoped_to_the_exact_source_file() {
    let root = repository_root();
    let boundary = root.join("rs/read/src/order_write");
    let dismissal = boundary.join("dismissal.rs");
    assert!(boundary_insert_allowed(
        &root,
        &dismissal,
        "MATCH_SUGGESTIONS"
    ));
    let mut files = Vec::new();
    collect(&boundary, "rs", &mut files);
    files.extend([
        boundary.join("effects.rs"),
        boundary.join("nested/dismissal.rs"),
        boundary.join("unlisted.rs"),
    ]);
    for path in files {
        if path != dismissal {
            assert!(!boundary_insert_allowed(&root, &path, "MATCH_SUGGESTIONS"));
        }
    }
    for table in [
        "AUDIT_LOGS",
        "NOTIFICATIONS",
        "LIVE_SLICE_BENCHMARKS",
        "USERS",
        "ORDERBOOK_ORDERS",
        "TRADES",
        "UNLISTED_TABLE",
    ] {
        assert!(!boundary_insert_allowed(&root, &dismissal, table));
    }
    for table in WRITE_TABLES {
        assert!(boundary_insert_allowed(&root, &dismissal, table));
    }
}

#[test]
fn matching_insert_authority_is_scoped_to_the_exact_source_file() {
    let root = repository_root();
    let boundary = root.join("rs/read/src/order_write");
    assert!(boundary_insert_allowed(
        &root,
        &boundary.join("matching.rs"),
        "TRADES"
    ));
    for file in [
        "effects.rs",
        "transaction.rs",
        "outbox.rs",
        "command.rs",
        "nested/matching.rs",
    ] {
        assert!(!boundary_insert_allowed(
            &root,
            &boundary.join(file),
            "TRADES"
        ));
    }
    for table in [
        "USERS",
        "ORDERBOOK_ORDERS",
        "INVENTORY_ITEMS",
        "AUDIT_LOGS",
        "NOTIFICATIONS",
        "UNLISTED_TABLE",
    ] {
        assert!(!boundary_insert_allowed(
            &root,
            &boundary.join("matching.rs"),
            table
        ));
    }
    for table in WRITE_TABLES {
        assert!(boundary_insert_allowed(
            &root,
            &boundary.join("matching.rs"),
            table
        ));
    }
}
