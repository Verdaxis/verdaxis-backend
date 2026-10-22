//! PD5: the watchlist producer activation comes only from release metadata.
//!
//! The serving router builds the watchlist settings once. Their producer
//! activation must be the instant a release recorded for this environment, or
//! no instant at all while no trusted producer is live. A clock reading or a
//! fixed default instant in its place would decide which stored events count
//! as trusted. These rules read the production sources, so a later change
//! that adds such a value fails here instead of passing review by omission.
use std::path::{Path, PathBuf};

/// The release metadata key the deploy transaction writes to record it.
const RELEASE_KEY: &str = "WATCHLIST_PRODUCER_ACTIVATED_AT";

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn collect(path: &Path, found: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(path).expect("readable source directory");
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            collect(&child, found);
        } else if child.extension().and_then(|value| value.to_str()) == Some("rs") {
            found.push(child);
        }
    }
}

/// Every production source file with its test module removed. Test modules
/// come last in each file, so the text before `#[cfg(test)]` is production.
fn production_sources() -> Vec<(PathBuf, String)> {
    let mut paths = Vec::new();
    collect(&source_root(), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("readable Rust source");
            let production = match text.find("#[cfg(test)]") {
                Some(start) => text[..start].to_owned(),
                None => text,
            };
            (path, production)
        })
        .collect()
}

/// The argument text of each call that starts with `call`, up to its
/// matching closing parenthesis.
fn call_arguments(text: &str, call: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (start, _) in text.match_indices(call) {
        let arguments = &text[start + call.len()..];
        let mut depth = 1;
        let end = arguments
            .char_indices()
            .find_map(|(index, character)| {
                match character {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(index)
            })
            .expect("a closed argument list");
        found.push(arguments[..end].to_owned());
    }
    found
}

/// Text that would put a clock reading or a fixed instant in the place of a
/// recorded activation.
const CLOCK_OR_DEFAULT: &[&str] = &[
    "now(",
    "UNIX_EPOCH",
    "MIN_UTC",
    "MAX_UTC",
    "default(",
    "Default",
    "from_timestamp",
    "with_ymd_and_hms",
    "SystemTime",
    "Instant",
];

fn relative(path: &Path) -> String {
    path.strip_prefix(source_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[test]
fn the_serving_router_builds_watchlist_settings_from_release_metadata() {
    let calls: Vec<(String, String)> = production_sources()
        .iter()
        .flat_map(|(path, text)| {
            call_arguments(text, "WatchlistSettings::new(")
                .into_iter()
                .map(move |arguments| (relative(path), arguments))
        })
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "exactly one production composition: {calls:?}"
    );
    let (path, arguments) = &calls[0];
    assert_eq!(path, "runtime/http.rs");
    assert!(
        arguments.contains("settings.watchlist_activation"),
        "the activation must be the release metadata value: {arguments}"
    );
    for forbidden in CLOCK_OR_DEFAULT {
        assert!(
            !arguments.contains(forbidden),
            "the watchlist settings take `{forbidden}`: {arguments}"
        );
    }
}

#[test]
fn the_activation_is_parsed_only_from_its_release_metadata_key() {
    let sources = production_sources();
    let text_of = |name: &str| {
        sources
            .iter()
            .find(|(path, _)| relative(path) == name)
            .map(|(_, text)| text.as_str())
            .unwrap_or_else(|| panic!("{name} is a production source"))
    };
    // The deployed environment reads the key that only the release metadata
    // file carries; the runtime preparer never projects it.
    let config = text_of("runtime/config.rs");
    assert!(
        config.contains(&format!(".get({RELEASE_KEY:?})")),
        "runtime/config.rs must read {RELEASE_KEY}"
    );
    // The settings parse that value and nothing else; no clock is consulted.
    let phase2 = text_of("runtime/phase2.rs");
    assert!(
        phase2.contains("watchlist_producer_activated_at"),
        "runtime/phase2.rs must parse the recorded activation"
    );
    for forbidden in CLOCK_OR_DEFAULT {
        assert!(
            !phase2.contains(forbidden),
            "runtime/phase2.rs consults `{forbidden}`"
        );
    }
    // No other production file names the key.
    let named: Vec<String> = sources
        .iter()
        .filter(|(_, text)| text.contains(RELEASE_KEY))
        .map(|(path, _)| relative(path))
        .collect();
    assert_eq!(named, vec!["runtime/config.rs", "runtime/phase2.rs"]);
}

#[test]
fn the_scan_finds_the_calls_it_checks() {
    let sample = "let a = WatchlistSettings::new(v.clone(), f(x));\n\
                  let b = WatchlistSettings::new(v, chrono::Utc::now());";
    let calls = call_arguments(sample, "WatchlistSettings::new(");
    assert_eq!(calls, vec!["v.clone(), f(x)", "v, chrono::Utc::now()"]);
    assert!(CLOCK_OR_DEFAULT
        .iter()
        .any(|token| calls[1].contains(token)));
}
