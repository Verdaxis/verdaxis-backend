//! Phase 4 contract inventory. Case IDs remain stable across implementation lanes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const BASE: &[&str] = &[
    "happy",
    "schema",
    "malformed",
    "wrong_method",
    "slash",
    "extra_path",
    "error_envelope",
    "source_owner",
];
const EXPECTED: &[(&str, &str, &str, &str)] = &[
    ("A01", "GET", "/api/auth/me", "B"),
    ("A02", "PUT", "/api/auth/me", "B"),
    ("A03", "POST", "/api/auth/survey", "B"),
    (
        "A04",
        "GET",
        "/api/auth/admin/invitations/organizations",
        "B",
    ),
    ("A05", "POST", "/api/auth/admin/invitations", "B"),
    ("A06", "POST", "/api/auth/invitations/resolve", "B"),
    ("A07", "PUT", "/api/auth/approve/{user_id}", "B"),
    ("A08", "PUT", "/api/auth/reject/{user_id}", "B"),
    (
        "A09",
        "PUT",
        "/api/auth/organization/{organization_id}/approve",
        "B",
    ),
    (
        "A10",
        "PUT",
        "/api/auth/organization/{organization_id}/reject",
        "B",
    ),
    ("A11", "GET", "/api/auth/admin/review-queue", "B"),
    ("A12", "GET", "/api/auth/admin/review-queue/{user_id}", "B"),
    ("A13", "GET", "/api/auth/organization-joins", "B"),
    (
        "A14",
        "PUT",
        "/api/auth/organization-joins/{join_request_id}/approve",
        "B",
    ),
    (
        "A15",
        "PUT",
        "/api/auth/organization-joins/{join_request_id}/reject",
        "B",
    ),
    ("K01", "POST", "/api/kyc/submit", "B"),
    ("K02", "GET", "/api/kyc/status", "B"),
    ("K03", "GET", "/api/kyc/admin/{user_id}/documents", "B"),
    ("K04", "GET", "/api/kyc/admin/documents/{document_id}", "B"),
    ("K05", "PUT", "/api/kyc/admin/{user_id}/approve", "B"),
    ("K06", "PUT", "/api/kyc/admin/{user_id}/reject", "B"),
    (
        "N01",
        "PUT",
        "/api/admin/analytics/users/{user_id}/reject",
        "B",
    ),
    ("N02", "GET", "/api/admin/onboarding-attention", "O"),
    ("N03", "GET", "/api/admin/audit-logs", "O"),
    ("V01", "GET", "/api/vessels", "V"),
    ("V02", "GET", "/api/vessels/{vessel_id}", "V"),
    ("S01", "GET", "/api/subscriptions/fees", "S"),
    ("S02", "GET", "/api/subscriptions/me", "S"),
    ("S03", "GET", "/api/admin/subscriptions", "S"),
    ("S04", "GET", "/api/admin/subscriptions/{org_id}", "S"),
    ("S05", "PUT", "/api/admin/subscriptions/{org_id}", "S"),
    ("P01", "GET", "/api/admin/analytics/product-usage", "P"),
    ("P02", "GET", "/api/admin/analytics/overview", "P"),
    ("P03", "GET", "/api/admin/analytics/daily", "P"),
    ("P04", "GET", "/api/admin/analytics/users", "P"),
    (
        "P05",
        "GET",
        "/api/admin/analytics/product-analytics/overview",
        "P",
    ),
    (
        "P06",
        "GET",
        "/api/admin/analytics/product-analytics/acquisition",
        "P",
    ),
    (
        "P07",
        "GET",
        "/api/admin/analytics/product-analytics/activation",
        "P",
    ),
    (
        "P08",
        "GET",
        "/api/admin/analytics/product-analytics/engagement",
        "P",
    ),
    (
        "P09",
        "GET",
        "/api/admin/analytics/product-analytics/marketplace",
        "P",
    ),
    (
        "P10",
        "GET",
        "/api/admin/analytics/product-analytics/retention",
        "P",
    ),
    (
        "P11",
        "GET",
        "/api/admin/analytics/product-analytics/reliability",
        "P",
    ),
];

fn text(path: &str) -> String {
    std::fs::read_to_string(Path::new(ROOT).join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn load(name: &str) -> Value {
    serde_json::from_str(&text(&format!("tests/rust_contract/phase4/{name}.json"))).unwrap()
}

fn rows<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key]
        .as_array()
        .unwrap_or_else(|| panic!("missing array {key}"))
        .as_slice()
}

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string {key}"))
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn strings(values: &[Value]) -> BTreeSet<&str> {
    let set: BTreeSet<_> = values.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(set.len(), values.len(), "duplicate entry");
    set
}

fn cells(route: &Value) -> BTreeSet<String> {
    let mut result: BTreeSet<String> = BASE.iter().map(|s| (*s).into()).collect();
    match string(route, "auth") {
        "P" => {
            result.insert("public_access".into());
        }
        "U" | "A" => {
            result.extend(["anonymous_401", "cutoff", "account_state"].map(String::from));
            if route["auth"] == "A" {
                result.insert("nonadmin_403".into());
            }
        }
        other => panic!("unknown auth {other}"),
    }
    let limits = rows(route, "limits");
    if limits.is_empty() {
        result.insert("no_route_limit".into());
    }
    for limit in limits {
        let id = string(limit, "id");
        result.insert(format!("{id}_accepted_at_limit"));
        result.insert(format!("{id}_429_retry_after"));
    }
    for scope in rows(route, "scopes") {
        for case in ["own", "foreign", "orgless"] {
            result.insert(format!("scope_{}_{case}", scope.as_str().unwrap()));
        }
    }
    if route["body"] != "none" || !rows(route, "query").is_empty() {
        result.extend(["input_boundary", "input_invalid"].map(String::from));
    }
    if route["writes"] == true {
        result.extend(["audit", "outbox", "rollback", "cancellation"].map(String::from));
    }
    for provider in rows(route, "providers") {
        for case in [
            "configured",
            "disabled",
            "busy",
            "timeout",
            "malformed",
            "unavailable",
        ] {
            result.insert(format!("provider_{}_{case}", provider.as_str().unwrap()));
        }
    }
    if route["database"] == true {
        result.insert("database_failure".into());
    }
    if !route["read_cap"].is_null() {
        result
            .extend(["filter_before_limit", "truncation_marker", "offset_bound"].map(String::from));
    }
    result
}

#[test]
fn p4_t1_c01() {
    let manifest = load("routes");
    let routes = rows(&manifest, "routes");
    assert_eq!(routes.len(), 42);
    let expected: BTreeSet<_> = EXPECTED.iter().copied().collect();
    let actual: BTreeSet<_> = routes
        .iter()
        .map(|r| {
            (
                string(r, "id"),
                string(r, "method"),
                string(r, "path"),
                string(r, "group"),
            )
        })
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(
        routes
            .iter()
            .map(|r| (string(r, "method"), string(r, "path")))
            .collect::<BTreeSet<_>>()
            .len(),
        42
    );
    assert_eq!(manifest["source_sha"].as_str().unwrap().len(), 40);
    let groups: BTreeMap<_, _> = [("B", 22), ("O", 2), ("V", 2), ("S", 5), ("P", 11)].into();
    for group in ["B", "O", "V", "S", "P"] {
        assert_eq!(
            routes.iter().filter(|r| r["group"] == *group).count(),
            groups[group]
        );
    }
    for source in rows(&manifest, "sources") {
        assert_eq!(
            digest(text(string(source, "path")).as_bytes()),
            string(source, "sha256")
        );
    }
    let client = load("client-contract");
    let surfaces = rows(&client, "surfaces");
    assert_eq!(surfaces.len(), 42);
    assert_eq!(
        surfaces
            .iter()
            .map(|r| string(r, "id"))
            .collect::<BTreeSet<_>>(),
        routes.iter().map(|r| string(r, "id")).collect()
    );
    for route in routes {
        let id = string(route, "id");
        let surface = surfaces.iter().find(|s| s["id"] == id).unwrap();
        assert_eq!(surface["method"], route["method"]);
        assert_eq!(surface["path"], route["path"]);
        let response_schema = string(surface, "response_schema");
        assert!(client["schemas"].get(response_schema).is_some());
        assert!(!rows(surface, "statuses").is_empty());
        assert_eq!(surface["error_schema"], "Error");
        assert_eq!(route["auth"] == "P", ["A06", "S01"].contains(&id));
        if ["A04", "A13", "N02", "S03"].contains(&id) {
            assert_eq!(route["read_cap"]["limit_default"], 100);
            assert_eq!(route["read_cap"]["limit_max"], 500);
            assert_eq!(route["read_cap"]["offset_max"], 10000);
            assert_eq!(route["read_cap"]["header"], "X-Result-Truncated");
        } else {
            assert!(route["read_cap"].is_null());
        }
        assert!(matches!(
            string(route, "body"),
            "none" | "json" | "multipart"
        ));
        if route["body"] != "none" {
            let request_schema = string(route, "request_schema");
            assert!(client["schemas"].get(request_schema).is_some());
        }
    }
}

#[test]
fn p4_t1_c02() {
    let manifest = load("invariants");
    let invariants = rows(&manifest, "invariants");
    let ids: BTreeSet<_> = invariants.iter().map(|v| string(v, "id")).collect();
    assert_eq!(ids.len(), invariants.len());
    for invariant in invariants {
        let id = string(invariant, "id");
        assert!(
            id.len() == 10
                && id.starts_with("P4_INV_")
                && id[7..].bytes().all(|b| b.is_ascii_digit())
        );
        assert!(!string(invariant, "statement").is_empty());
        assert!(!rows(invariant, "evidence").is_empty());
        for evidence in rows(invariant, "evidence") {
            match string(evidence, "kind") {
                "rust_test" => {
                    let case = string(evidence, "case");
                    assert!(
                        case.starts_with("p4_t")
                            || (evidence["target"] == "phase4_db_authority"
                                && include_str!("phase4_db_authority.rs")
                                    .contains(&format!("async fn {case}()")))
                    );
                }
                "preserved_invariant" => {
                    assert!(!string(evidence, "owner").is_empty());
                }
                "phase10_owner" => {
                    assert_eq!(evidence["owner"], "phase10");
                }
                other => panic!("unknown disposition {other}"),
            }
        }
    }
    let files = rows(&manifest, "source_files");
    assert_eq!(files.len(), 41);
    let mut definitions = BTreeSet::new();
    let mut unit = 0;
    let mut postgres = 0;
    for file in files {
        let source = text(string(file, "path"));
        assert_eq!(digest(source.as_bytes()), string(file, "sha256"));
        let count = source
            .lines()
            .filter(|line| {
                line.trim_start()
                    .strip_prefix("async ")
                    .unwrap_or(line.trim_start())
                    .starts_with("def test_")
            })
            .count();
        let dispositions = rows(file, "definitions");
        assert_eq!(dispositions.len(), count);
        for (index, definition) in dispositions.iter().enumerate() {
            assert_eq!(definition["ordinal"], index + 1);
            assert!(definitions.insert(string(definition, "id")));
            assert!(ids.contains(string(definition, "invariant")));
            let parameters = rows(definition, "parameter_rules");
            assert_eq!(
                parameters.len() as u64,
                definition["parameter_rule_count"].as_u64().unwrap()
            );
            assert_eq!(strings(parameters).len(), parameters.len());
            assert_eq!(string(definition, "parameter_sha256").len(), 64);
        }
        if string(file, "path").starts_with("tests/postgres/") {
            postgres += count;
        } else {
            unit += count;
        }
    }
    assert_eq!((definitions.len(), unit, postgres), (287, 268, 19));
}

#[test]
fn p4_t1_c03() {
    let manifest = load("evidence-manifest");
    let routes = load("routes");
    for route in rows(&routes, "routes") {
        for excluded in rows(&manifest, "excluded_prefixes") {
            let path = string(route, "path");
            let prefix = excluded.as_str().unwrap();
            assert!(path != prefix && !path.starts_with(&format!("{prefix}/")));
        }
    }
    let source = text("app/routers/market_support.py");
    let count = source
        .lines()
        .filter(|line| line.trim_start().starts_with("@router."))
        .count();
    assert_eq!(count, 16);
    assert_eq!(manifest["excluded_market_support_routes"], count);
}

#[test]
fn p4_t1_c04() {
    let routes = load("routes");
    let manifest = load("evidence-manifest");
    assert_eq!(
        strings(rows(&manifest, "base_cells")),
        BASE.iter().copied().collect()
    );
    assert_eq!(manifest["measurement_cells"], false);
    let mut expected = BTreeSet::new();
    for route in rows(&routes, "routes") {
        let id = string(route, "id");
        let required = cells(route);
        let scoped = [
            "A07", "A08", "A09", "A10", "A12", "A13", "A14", "A15", "K01", "K03", "K04", "K05",
            "K06", "N01", "V01", "V02", "S02", "S04", "S05",
        ];
        assert_eq!(
            !rows(route, "scopes").is_empty(),
            scoped.contains(&id),
            "{id}"
        );
        let provider_routes = [
            "A07", "K05", "K06", "I01", "P01", "P05", "P06", "P07", "P08", "P10", "P11",
        ];
        assert_eq!(
            !rows(route, "providers").is_empty(),
            provider_routes.contains(&id),
            "{id}"
        );
        let writes = (route["method"] != "GET" && !["A06", "I01"].contains(&id))
            || ["K04", "S02"].contains(&id);
        assert_eq!(route["writes"], writes, "{id}");
        assert_eq!(route["database"], id != "S01", "{id}");
        assert_eq!(required, cells(route));
        for cell in required {
            expected.insert(format!("{id}:{cell}"));
        }
        let limits = rows(route, "limits");
        let required_limits: &[(u64, &str, &str)] = match id {
            "A01" => &[(120, "minute", "client")],
            "A02" => &[(10, "minute", "client")],
            "A03" => &[(5, "minute", "client")],
            "A05" => &[(30, "minute", "client")],
            "A06" => &[(20, "minute", "client")],
            "K01" => &[(5, "hour", "user")],
            "K03" | "K04" => &[(30, "minute", "client")],
            "I01" => &[(10, "minute", "user"), (30, "minute", "org")],
            "N01" | "P02" | "P03" | "P04" => &[(60, "minute", "token")],
            "P01" | "P05" | "P06" | "P07" | "P08" | "P09" | "P10" | "P11" => {
                &[(30, "minute", "token")]
            }
            "A04" | "A07" | "A08" | "A09" | "A10" | "A11" | "A12" | "A13" | "A14" | "A15"
            | "N02" => &[(60, "minute", "client")],
            _ => &[],
        };
        assert_eq!(limits.len(), required_limits.len(), "{id}");
        for (limit, (count, window, key)) in limits.iter().zip(required_limits) {
            assert_eq!(limit["count"], *count, "{id}");
            assert_eq!(limit["window"], *window, "{id}");
            assert_eq!(limit["key"], *key, "{id}");
        }
    }
    let declared = rows(&manifest, "required_cells");
    assert_eq!(
        strings(declared),
        expected.iter().map(String::as_str).collect()
    );
    for name in [
        "routes",
        "client-contract",
        "invariants",
        "evidence-manifest",
    ] {
        let source = text(&format!("tests/rust_contract/phase4/{name}.json"));
        for forbidden in [
            "P4-D",
            "defect",
            "exploit",
            "CVE-",
            "byte-identical",
            "twin-database",
        ] {
            assert!(!source.contains(forbidden), "{name}: {forbidden}");
        }
    }
}
