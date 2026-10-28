//! The heartbeat frame and its configuration: no database and no router.
//!
//! The stream-level behaviour (first heartbeat, cadence under traffic, order
//! against control frames) is held in `streams_replay.rs` and
//! `streams_http.rs`.

use std::convert::Infallible;

use axum::{
    body::to_bytes,
    response::{IntoResponse, sse::Sse},
};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use verdaxis_read::streams::heartbeat::{HeartbeatStatus, MAX_MESSAGE_CHARS, SystemStatus};

fn at(seconds: i64, nanos: u32) -> DateTime<Utc> {
    Utc.timestamp_opt(seconds, nanos).single().unwrap()
}

/// The bytes the SSE encoder writes for one heartbeat.
async fn wire(status: &HeartbeatStatus, now: DateTime<Utc>) -> String {
    let frame = Ok::<_, Infallible>(status.event(now));
    let response = Sse::new(futures_util::stream::iter([frame])).into_response();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn parse(status: Option<&str>, message: Option<&str>) -> Result<HeartbeatStatus, String> {
    HeartbeatStatus::parse(status, message)
}

#[tokio::test]
async fn frame_is_a_named_event_with_one_json_line_and_no_id() {
    let status = parse(Some("degraded"), Some("Matching is slow")).unwrap();
    // 2026-10-28T12:34:56.789Z: the fraction is dropped, not rounded.
    let text = wire(&status, at(1_793_190_896, 789_000_000)).await;
    assert!(text.ends_with("\n\n"), "{text:?}");
    let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
    assert_eq!(lines.len(), 2, "{text:?}");
    assert_eq!(lines[0], "event: heartbeat");
    assert!(!text.lines().any(|line| line.starts_with("id")), "{text:?}");
    assert!(!text.lines().any(|line| line.starts_with(':')), "{text:?}");
    let data: Value = serde_json::from_str(lines[1].strip_prefix("data: ").unwrap()).unwrap();
    assert_eq!(
        data,
        json!({
            "status": "degraded",
            "message": "Matching is slow",
            "server_time": "2026-10-28T12:34:56Z",
        })
    );
}

#[tokio::test]
async fn fields_keep_the_documented_order() {
    let text = wire(&HeartbeatStatus::default(), at(1_793_190_896, 0)).await;
    assert_eq!(
        text,
        "event: heartbeat\n\
         data: {\"status\":\"operational\",\"message\":null,\"server_time\":\"2026-10-28T12:34:56Z\"}\n\n"
    );
}

#[tokio::test]
async fn a_message_cannot_break_the_frame() {
    // Line breaks, a forged field, a NUL, quotes and the Unicode line
    // separators: all stay inside one JSON string on one data line.
    let message = "a\r\nid: 9\n\nevent: reset\ndata: {}\u{0}\"q\"\u{2028}\u{2029}\\";
    let status = parse(Some("maintenance"), Some(message)).unwrap();
    let text = wire(&status, at(1_793_190_896, 0)).await;
    let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
    assert_eq!(lines.len(), 2, "{text:?}");
    assert!(!text.contains('\r'), "{text:?}");
    let data: Value = serde_json::from_str(lines[1].strip_prefix("data: ").unwrap()).unwrap();
    assert_eq!(data["message"], message);
    assert_eq!(data["status"], "maintenance");
}

#[test]
fn absent_status_is_operational_and_absent_message_is_null() {
    let status = parse(None, None).unwrap();
    assert_eq!(status.status(), SystemStatus::Operational);
    assert_eq!(status.message(), None);
    assert_eq!(status, HeartbeatStatus::default());
}

#[test]
fn status_is_trimmed_and_lowercased() {
    for (text, expected) in [
        ("operational", SystemStatus::Operational),
        ("  Degraded\t", SystemStatus::Degraded),
        ("MAINTENANCE", SystemStatus::Maintenance),
    ] {
        assert_eq!(
            parse(Some(text), None).unwrap().status(),
            expected,
            "{text:?}"
        );
    }
}

#[test]
fn any_other_status_is_refused() {
    for text in [
        "",
        "   ",
        "ok",
        "down",
        "operational,degraded",
        "degraded\nx",
        "maintenence",
    ] {
        let error = parse(Some(text), None).unwrap_err();
        assert!(error.contains("SYSTEM_STATUS"), "{text:?}: {error}");
    }
}

#[test]
fn message_is_trimmed_and_blank_is_null() {
    assert_eq!(
        parse(None, Some("  Back at noon \n")).unwrap().message(),
        Some("Back at noon")
    );
    for blank in ["", "   ", "\n\t "] {
        assert_eq!(
            parse(None, Some(blank)).unwrap().message(),
            None,
            "{blank:?}"
        );
    }
}

#[test]
fn message_limit_counts_characters() {
    let exact = "x".repeat(MAX_MESSAGE_CHARS);
    assert!(parse(None, Some(&exact)).is_ok());
    // 280 four-byte characters are within the limit; 281 are not.
    let wide = "\u{1F6A2}".repeat(MAX_MESSAGE_CHARS);
    assert!(parse(None, Some(&wide)).is_ok());
    for over in [format!("{exact}x"), format!("{wide}\u{1F6A2}")] {
        let error = parse(None, Some(&over)).unwrap_err();
        assert!(error.contains("SYSTEM_STATUS_MESSAGE"), "{error}");
    }
    // The limit applies after trimming.
    assert!(parse(None, Some(&format!("  {exact}  "))).is_ok());
}
