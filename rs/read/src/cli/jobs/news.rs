//! `verdaxis-read job news`: one database-locked news refresh per timer run.
//!
//! The order is the one the Python job used. A transaction takes the
//! transaction-scoped advisory lock (a busy lock is a successful no-op), the
//! job reads one bounded Webz.io page when a token is configured and falls back
//! to the maritime RSS feeds when Webz is absent, empty or unavailable, drops
//! duplicates, labels only the URLs that are not stored yet and inserts them
//! with `ON CONFLICT (url) DO NOTHING`.
//!
//! Three transports sit behind small traits so the whole flow runs against
//! fakes: [`Fetch`] (bounded HTTP GET), [`TextModel`] (one headline label) and
//! the PostgreSQL pool. No type here prints a token, a provider body or a
//! database URL.
//!
//! Differences from the Python job, all on purpose:
//! - the command needs `--apply` like every other writing command;
//! - an empty `WEBZ_API_TOKEN` counts as no token;
//! - a headline whose source URL would not fit the `source_url` column is
//!   skipped instead of failing the whole insert;
//! - a model-written summary meets the same plain-text and length rule as a
//!   provider summary before it is stored;
//! - IP literals are judged by the special-purpose address registry and by the
//!   WHATWG host parser a browser uses, so odd spellings such as `0x7f.1` are
//!   refused rather than treated as host names.

use super::super::context::{self, Purpose, RunContext};
use super::super::gate::{require_apply, require_job_preflight, require_target};
use super::super::{Environment, Exit, JobArgs, JobKind};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::{Host, Url};
use uuid::Uuid;

/// The advisory key the Python job used. It must stay identical while the
/// Python command can still run, so the two never overlap.
pub const NEWS_REFRESH_ADVISORY_LOCK_ID: i64 = 6_216_461_178_696_259_923;

pub const NEWS_PROVIDER_TIMEOUT: Duration = Duration::from_secs(15);
pub const RSS_FETCH_TIMEOUT: Duration = Duration::from_secs(15);
pub const RSS_MAX_CONCURRENT: usize = 4;
pub const RSS_MAX_BYTES_PER_FEED: usize = 2 * 1024 * 1024;
pub const RSS_MAX_ENTRIES_PER_FEED: usize = 100;
pub const RSS_MAX_ENTRIES_PER_RUN: usize = 250;
pub const NEWS_MAX_PROVIDER_CALLS_PER_RUN: u32 = 50;
pub const NEWS_TITLE_MAX_CHARS: usize = 500;
pub const NEWS_URL_MAX_CHARS: usize = 1000;
pub const NEWS_SOURCE_MAX_CHARS: usize = 100;
pub const NEWS_SOURCE_URL_MAX_CHARS: usize = 500;
pub const NEWS_SUMMARY_MAX_CHARS: usize = 1000;
pub const WEBZ_MAX_ENTRIES_PER_RUN: usize = 100;
pub const WEBZ_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const WEBZ_DEFAULT_URL: &str = "https://api.webz.io/filterWebContent";
pub const WEBZ_MAX_TIMEOUT_SECONDS: f64 = 30.0;
pub const GEMINI_MODEL: &str = "gemini-2.5-flash-lite";
pub const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const GEMINI_ATTEMPTS: u32 = 2;
const GEMINI_RETRY_DELAY: Duration = Duration::from_millis(100);
const GEMINI_RESPONSE_MAX_BYTES: usize = 1024 * 1024;
const BREAKER_THRESHOLD: u32 = 3;
const BREAKER_COOLDOWN: Duration = Duration::from_secs(60);

const WEBZ_FUELS: &str = "(\"bio methanol\" OR biomethanol OR \"e-methanol\" OR \"e methanol\" OR \
\"synthetic methanol\" OR \"bio ethanol\" OR bioethanol OR \"e-ethanol\" \
OR \"e ethanol\" OR \"synthetic ethanol\")";
const WEBZ_MARITIME: &str = "(marine OR maritime OR bunker OR bunkering OR shipping)";
const WEBZ_REGULATION: &str = "(\"FuelEU Maritime\" OR \"marine EU ETS\" OR \"IMO GFI\" OR \
\"marine fuel regulation\")";

const VALID_CATEGORIES: [&str; 6] = [
    "shipping",
    "bunkers",
    "regulation",
    "carbon",
    "commodities",
    "markets",
];

/// The Webz.io search expression, word for word what the Python job sent.
pub fn webz_query() -> String {
    format!(
        "site_type:news AND (((title:{WEBZ_FUELS} OR summary:{WEBZ_FUELS}) \
AND (title:{WEBZ_MARITIME} OR summary:{WEBZ_MARITIME})) OR \
title:{WEBZ_REGULATION} OR summary:{WEBZ_REGULATION})"
    )
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// A credential that cannot reach a log through `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebzConfig {
    pub token: Secret,
    pub url: String,
    pub timeout: Duration,
}

/// The settings the job reads. Task 9 projects them into the job environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewsConfig {
    pub webz: Option<WebzConfig>,
    pub gemini_api_key: Option<Secret>,
}

impl NewsConfig {
    /// Read `WEBZ_API_TOKEN`, `WEBZ_API_URL`, `WEBZ_API_TIMEOUT_SECONDS` and
    /// `GEMINI_API_KEY`. An invalid URL or timeout is a precondition refusal
    /// before any network or database work.
    pub fn from_environment(environment: &HashMap<String, String>) -> Result<Self, Exit> {
        let non_empty = |key: &str| {
            environment
                .get(key)
                .filter(|value| !value.is_empty())
                .cloned()
        };
        let url = match environment.get("WEBZ_API_URL") {
            Some(value) => validated_webz_url(value)?,
            None => WEBZ_DEFAULT_URL.to_owned(),
        };
        let timeout = match environment.get("WEBZ_API_TIMEOUT_SECONDS") {
            Some(value) => validated_webz_timeout(value)?,
            None => NEWS_PROVIDER_TIMEOUT,
        };
        Ok(Self {
            webz: non_empty("WEBZ_API_TOKEN").map(|token| WebzConfig {
                token: Secret::new(token),
                url,
                timeout,
            }),
            gemini_api_key: non_empty("GEMINI_API_KEY").map(Secret::new),
        })
    }
}

/// Only the licensed Webz.io endpoint is allowed: the token goes in the query.
fn validated_webz_url(value: &str) -> Result<String, Exit> {
    let normalized = value.trim();
    let parsed = Url::parse(normalized).map_err(|_| Exit::Precondition)?;
    let exact = parsed.scheme() == "https"
        && parsed.host_str() == Some("api.webz.io")
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.port().is_none()
        && parsed.path() == "/filterWebContent"
        && parsed.query().is_none()
        && parsed.fragment().is_none();
    if exact {
        Ok(normalized.to_owned())
    } else {
        Err(Exit::Precondition)
    }
}

fn validated_webz_timeout(value: &str) -> Result<Duration, Exit> {
    let seconds: f64 = value.trim().parse().map_err(|_| Exit::Precondition)?;
    if seconds.is_finite() && seconds > 0.0 && seconds <= WEBZ_MAX_TIMEOUT_SECONDS {
        Ok(Duration::from_secs_f64(seconds))
    } else {
        Err(Exit::Precondition)
    }
}

// ---------------------------------------------------------------------------
// Candidate items and their validation
// ---------------------------------------------------------------------------

/// One validated headline, not yet stored or labelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub title: String,
    pub url: String,
    pub source: String,
    pub source_url: String,
    pub published_at: DateTime<Utc>,
    pub summary: Option<String>,
}

fn has_control_character(value: &str) -> bool {
    value
        .chars()
        .any(|character| (character as u32) < 0x20 || character as u32 == 0x7F)
}

/// Host-name suffixes that name private networks.
const LOCAL_SUFFIXES: [&str; 5] = [".localhost", ".local", ".internal", ".home", ".lan"];

/// Whether an address is globally routable. The tables are the IANA
/// special-purpose registries; multicast is refused as well, since it is never
/// a publisher address.
pub fn is_global_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_global_v4(v4),
        IpAddr::V6(v6) => is_global_v6(v6),
    }
}

fn is_global_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(a == 0
        || a == 10
        || (a == 100 && (64..=127).contains(&b))
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_global_v6(address: Ipv6Addr) -> bool {
    if let Some(embedded) = address.to_ipv4_mapped() {
        return is_global_v4(embedded);
    }
    let segments = address.segments();
    let nat64 = segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0];
    if nat64 {
        let [high, low] = [segments[6], segments[7]];
        return is_global_v4(Ipv4Addr::new(
            (high >> 8) as u8,
            high as u8,
            (low >> 8) as u8,
            low as u8,
        ));
    }
    let first = segments[0];
    !(address.is_unspecified()
        || address.is_loopback()
        || (first == 0x64 && segments[1] == 0xff9b && segments[2] == 1)
        || (first == 0x100 && segments[1..4] == [0, 0, 0])
        || (first == 0x2001 && segments[1] < 0x200)
        || (first == 0x2001 && segments[1] == 0xdb8)
        || first == 0x2002
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || (first & 0xff00) == 0xff00)
}

/// A bounded public `http(s)` URL that is safe to show as a browser link.
pub fn validated_public_http_url(raw: &str, max_chars: usize) -> Option<String> {
    let candidate = raw.trim();
    if candidate.is_empty() || candidate.chars().count() > max_chars {
        return None;
    }
    if candidate.chars().any(|character| {
        character.is_whitespace() || (character as u32) < 0x20 || character == '\u{7f}'
    }) {
        return None;
    }
    let (_, authority) = split_authority(candidate)?;
    if authority.contains('@') || authority.contains('\\') {
        return None;
    }
    let parsed = Url::parse(candidate).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let public = match parsed.host()? {
        Host::Domain(domain) => {
            let hostname = domain.trim_end_matches('.').to_lowercase();
            !hostname.is_empty()
                && hostname != "localhost"
                && !LOCAL_SUFFIXES
                    .iter()
                    .any(|suffix| hostname.ends_with(suffix))
        }
        Host::Ipv4(address) => is_global_v4(address),
        Host::Ipv6(address) => is_global_v6(address),
    };
    public.then(|| candidate.to_owned())
}

/// Split `scheme://authority/...` into its scheme and authority. A URL with no
/// `//` after the scheme has no authority, as `urlsplit` reads it.
fn split_authority(candidate: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = candidate.split_once(':')?;
    let rest = rest.strip_prefix("//")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some((scheme, &rest[..end]))
}

fn hostname_of(url: &str) -> Option<String> {
    match Url::parse(url).ok()?.host()? {
        Host::Domain(domain) => Some(domain.trim_end_matches('.').to_lowercase()),
        Host::Ipv4(address) => Some(address.to_string()),
        Host::Ipv6(address) => Some(address.to_string()),
    }
}

/// A public URL that belongs to the configured publisher.
pub fn validated_article_url(raw: &str, allowed_domains: &[String]) -> Option<String> {
    let candidate = validated_public_http_url(raw, NEWS_URL_MAX_CHARS)?;
    let hostname = hostname_of(&candidate)?;
    allowed_domains
        .iter()
        .any(|domain| hostname == *domain || hostname.ends_with(&format!(".{domain}")))
        .then_some(candidate)
}

/// Plain text with no control characters (line breaks only when allowed),
/// whitespace collapsed, non-empty and at most `max_chars` characters.
pub fn bounded_plain_text(
    raw: &Value,
    max_chars: usize,
    allow_line_breaks: bool,
) -> Option<String> {
    let raw = raw.as_str()?;
    let forbidden = |character: char| {
        let code = character as u32;
        (code < 0x20 && !(allow_line_breaks && matches!(character, '\t' | '\r' | '\n')))
            || code == 0x7F
    };
    if raw.chars().any(forbidden) {
        return None;
    }
    let value = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    (!value.is_empty() && value.chars().count() <= max_chars).then_some(value)
}

/// Python truthiness of a decoded JSON value, for the `a or b` fallbacks.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

fn first_truthy(candidates: [Option<&Value>; 2]) -> Option<&Value> {
    candidates
        .into_iter()
        .flatten()
        .find(|value| is_truthy(value))
}

// ---------------------------------------------------------------------------
// RSS and Atom feeds
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedSpec {
    pub name: String,
    pub url: String,
    pub article_domains: Vec<String>,
}

impl FeedSpec {
    fn new(name: &str, url: &str, domain: &str) -> Self {
        Self {
            name: name.to_owned(),
            url: url.to_owned(),
            article_domains: vec![domain.to_owned()],
        }
    }
}

/// The maritime publishers, in the order the Python job read them.
pub fn default_feeds() -> Vec<FeedSpec> {
    vec![
        FeedSpec::new(
            "TradeWinds",
            "https://www.tradewindsnews.com/rss",
            "tradewindsnews.com",
        ),
        FeedSpec::new(
            "Ship & Bunker",
            "https://shipandbunker.com/rss",
            "shipandbunker.com",
        ),
        FeedSpec::new("Splash247", "https://splash247.com/feed/", "splash247.com"),
        FeedSpec::new(
            "Maritime Executive",
            "https://maritime-executive.com/rss",
            "maritime-executive.com",
        ),
        FeedSpec::new(
            "Hellenic Shipping News",
            "https://www.hellenicshippingnews.com/feed/",
            "hellenicshippingnews.com",
        ),
    ]
}

/// Parse one feed document and keep the validated entries, at most
/// [`RSS_MAX_ENTRIES_PER_FEED`] read. `None` means the document did not parse.
pub fn parse_feed(body: &[u8], feed: &FeedSpec, now: DateTime<Utc>) -> Option<Vec<Candidate>> {
    let parsed = feed_rs::parser::parse(body).ok()?;
    Some(
        parsed
            .entries
            .into_iter()
            .take(RSS_MAX_ENTRIES_PER_FEED)
            .filter_map(|entry| validated_feed_entry(entry, feed, now))
            .collect(),
    )
}

fn validated_feed_entry(
    entry: feed_rs::model::Entry,
    feed: &FeedSpec,
    now: DateTime<Utc>,
) -> Option<Candidate> {
    let title = entry.title?.content.trim().to_owned();
    if title.is_empty()
        || title.chars().count() > NEWS_TITLE_MAX_CHARS
        || has_control_character(&title)
    {
        return None;
    }
    let link = entry
        .links
        .iter()
        .find(|link| link.rel.as_deref().is_none_or(|rel| rel == "alternate"))
        .or(entry.links.first())?;
    let url = validated_article_url(&link.href, &feed.article_domains)?;
    Some(Candidate {
        title,
        url,
        source: feed.name.clone(),
        source_url: feed.url.clone(),
        published_at: bounded_published(entry.published.or(entry.updated), now),
        summary: None,
    })
}

// ---------------------------------------------------------------------------
// Webz.io
// ---------------------------------------------------------------------------

/// A publication time the database and Python's `datetime` can hold (years 1
/// to 9999), else the run time.
pub fn bounded_published(published: Option<DateTime<Utc>>, now: DateTime<Utc>) -> DateTime<Utc> {
    published
        .filter(|time| (1..=9999).contains(&time.year()))
        .unwrap_or(now)
}

/// Read a Webz `published` value: epoch seconds or milliseconds, or an ISO
/// timestamp with an offset. Anything else is "now".
pub fn parse_webz_published(raw: Option<&Value>, now: DateTime<Utc>) -> DateTime<Utc> {
    const MAX_SECONDS: f64 = 253_402_300_799.0; // 9999-12-31T23:59:59Z
    match raw {
        Some(Value::Number(number)) => {
            let Some(value) = number.as_f64().filter(|value| *value >= 0.0) else {
                return now;
            };
            let seconds = if value > 10_000_000_000.0 {
                value / 1000.0
            } else {
                value
            };
            if !seconds.is_finite() || seconds > MAX_SECONDS {
                return now;
            }
            let whole = seconds.floor();
            let microseconds = ((seconds - whole) * 1_000_000.0).round() as u32;
            bounded_published(
                Utc.timestamp_opt(whole as i64, microseconds.min(999_999) * 1000)
                    .single(),
                now,
            )
        }
        Some(Value::String(text)) => bounded_published(parse_iso_with_offset(text.trim()), now),
        _ => now,
    }
}

fn parse_iso_with_offset(text: &str) -> Option<DateTime<Utc>> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Some(parsed.with_timezone(&Utc));
    }
    [
        "%Y-%m-%dT%H:%M:%S%.f%z",
        "%Y-%m-%d %H:%M:%S%.f%z",
        "%Y-%m-%d %H:%M:%S%.f%:z",
    ]
    .iter()
    .find_map(|format| DateTime::parse_from_str(text, format).ok())
    .map(|parsed| parsed.with_timezone(&Utc))
}

/// One Webz post as a candidate, or `None` when any required field is missing,
/// unbounded or unsafe.
pub fn validated_webz_post(post: &Value, now: DateTime<Utc>) -> Option<Candidate> {
    let post = post.as_object()?;
    let empty = serde_json::Map::new();
    let thread = post
        .get("thread")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    let title = bounded_plain_text(
        first_truthy([post.get("title"), thread.get("title")])?,
        NEWS_TITLE_MAX_CHARS,
        false,
    )?;
    let raw_url = first_truthy([post.get("url"), thread.get("url")])?.as_str()?;
    let url = validated_public_http_url(raw_url, NEWS_URL_MAX_CHARS)?;

    let (scheme, authority) = split_authority(&url)?;
    let hostname = hostname_of(&url)?;
    let source = match first_truthy([thread.get("site"), thread.get("site_full")]) {
        Some(value) => bounded_plain_text(value, NEWS_SOURCE_MAX_CHARS, false)?,
        None => bounded_plain_text(&Value::String(hostname), NEWS_SOURCE_MAX_CHARS, false)?,
    };
    let source_url = format!("{scheme}://{authority}");
    if source_url.chars().count() > NEWS_SOURCE_URL_MAX_CHARS {
        return None;
    }
    Some(Candidate {
        title,
        url,
        source,
        source_url,
        published_at: parse_webz_published(
            first_truthy([post.get("published"), thread.get("published")]),
            now,
        ),
        summary: post
            .get("summary")
            .and_then(|summary| bounded_plain_text(summary, NEWS_SUMMARY_MAX_CHARS, true)),
    })
}

/// Collapse duplicate URLs and same-publisher headlines within one batch,
/// keeping the first of each and at most [`RSS_MAX_ENTRIES_PER_RUN`].
pub fn dedupe_candidates(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let folder = icu_casemap::CaseMapper::new();
    let mut seen_urls = HashSet::new();
    let mut seen_titles = HashSet::new();
    let mut unique = Vec::new();
    for candidate in candidates {
        let source_title = (
            folder.fold_string(&candidate.source).into_owned(),
            folder.fold_string(&candidate.title).into_owned(),
        );
        if seen_urls.contains(&candidate.url) || seen_titles.contains(&source_title) {
            continue;
        }
        seen_urls.insert(candidate.url.clone());
        seen_titles.insert(source_title);
        unique.push(candidate);
        if unique.len() >= RSS_MAX_ENTRIES_PER_RUN {
            break;
        }
    }
    unique
}

// ---------------------------------------------------------------------------
// Bounded HTTP
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    /// The decoded body crossed the byte cap.
    TooLarge,
    /// Connection, timeout or a status other than 2xx. No detail is kept: a
    /// transport error can carry the request URL, and the Webz token is in it.
    Unavailable,
}

pub struct FetchRequest<'a> {
    pub url: &'a str,
    pub query: &'a [(&'static str, String)],
    pub max_bytes: usize,
    pub timeout: Duration,
}

/// One bounded `GET`. Redirects are not followed.
pub trait Fetch: Send + Sync {
    fn get(
        &self,
        request: FetchRequest<'_>,
    ) -> impl Future<Output = Result<Vec<u8>, FetchError>> + Send;
}

pub struct ReqwestFetch {
    client: reqwest::Client,
}

impl ReqwestFetch {
    pub fn new() -> Result<Self, Exit> {
        reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .map(|client| Self { client })
            .map_err(|_| Exit::Dependency)
    }
}

impl Fetch for ReqwestFetch {
    async fn get(&self, request: FetchRequest<'_>) -> Result<Vec<u8>, FetchError> {
        let mut url = Url::parse(request.url).map_err(|_| FetchError::Unavailable)?;
        if !request.query.is_empty() {
            url.query_pairs_mut().extend_pairs(
                request
                    .query
                    .iter()
                    .map(|(key, value)| (*key, value.as_str())),
            );
        }
        let builder = self.client.get(url).timeout(request.timeout);
        let mut response = builder.send().await.map_err(|_| FetchError::Unavailable)?;
        if !response.status().is_success() {
            return Err(FetchError::Unavailable);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| FetchError::Unavailable)?
        {
            if body.len() + chunk.len() > request.max_bytes {
                return Err(FetchError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

// ---------------------------------------------------------------------------
// Sources: Webz first, RSS as the fallback
// ---------------------------------------------------------------------------

pub struct NewsSources<F> {
    pub fetch: F,
    pub webz: Option<WebzConfig>,
    pub feeds: Vec<FeedSpec>,
}

impl<F: Fetch> NewsSources<F> {
    /// One bounded Webz page. `None` means the request failed; `Some(empty)`
    /// is a valid answer with no usable record.
    pub async fn fetch_webz(
        &self,
        webz: &WebzConfig,
        now: DateTime<Utc>,
    ) -> Option<Vec<Candidate>> {
        let query = [
            ("token", webz.token.expose().to_owned()),
            ("format", "json".to_owned()),
            ("q", webz_query()),
            ("sort", "published".to_owned()),
            ("order", "desc".to_owned()),
            ("size", WEBZ_MAX_ENTRIES_PER_RUN.to_string()),
            ("includeSyndicated", "false".to_owned()),
        ];
        let body = self
            .fetch
            .get(FetchRequest {
                url: &webz.url,
                query: &query,
                max_bytes: WEBZ_MAX_RESPONSE_BYTES,
                timeout: webz.timeout,
            })
            .await;
        let body = match body {
            Ok(body) => body,
            Err(FetchError::TooLarge) => {
                tracing::warn!(reason = "body_too_large", "news_feed.webz_unavailable");
                return None;
            }
            Err(FetchError::Unavailable) => {
                tracing::warn!(reason = "http_error", "news_feed.webz_unavailable");
                return None;
            }
        };
        let payload: Value = match serde_json::from_slice(&body) {
            Ok(payload) => payload,
            Err(_) => {
                tracing::warn!(reason = "invalid_response", "news_feed.webz_unavailable");
                return None;
            }
        };
        let Some(posts) = payload.get("posts").and_then(Value::as_array) else {
            tracing::warn!(reason = "invalid_response", "news_feed.webz_unavailable");
            return None;
        };
        let posts = &posts[..posts.len().min(WEBZ_MAX_ENTRIES_PER_RUN)];
        let items: Vec<Candidate> = posts
            .iter()
            .filter_map(|post| validated_webz_post(post, now))
            .collect();
        tracing::info!(
            fetched = posts.len(),
            accepted = items.len(),
            rejected = posts.len() - items.len(),
            "news_feed.webz_fetched"
        );
        Some(items)
    }

    async fn fetch_one_feed(&self, feed: &FeedSpec, now: DateTime<Utc>) -> Vec<Candidate> {
        let body = self
            .fetch
            .get(FetchRequest {
                url: &feed.url,
                query: &[],
                max_bytes: RSS_MAX_BYTES_PER_FEED,
                timeout: RSS_FETCH_TIMEOUT,
            })
            .await;
        let body = match body {
            Ok(body) => body,
            Err(FetchError::TooLarge) => {
                tracing::warn!(feed = %feed.name, error_type = "body_too_large", "news_feed.fetch_error");
                return Vec::new();
            }
            Err(FetchError::Unavailable) => {
                tracing::warn!(feed = %feed.name, error_type = "network_error", "news_feed.fetch_error");
                return Vec::new();
            }
        };
        parse_feed(&body, feed, now).unwrap_or_else(|| {
            tracing::warn!(feed = %feed.name, error_type = "parse_error", "news_feed.fetch_error");
            Vec::new()
        })
    }

    /// Read every feed, at most [`RSS_MAX_CONCURRENT`] at once, and keep their
    /// entries in feed order up to [`RSS_MAX_ENTRIES_PER_RUN`].
    pub async fn fetch_all_feeds(&self, now: DateTime<Utc>) -> Vec<Candidate> {
        let fetched: Vec<Vec<Candidate>> = futures_util::stream::iter(self.feeds.iter())
            .map(|feed| self.fetch_one_feed(feed, now))
            .buffered(RSS_MAX_CONCURRENT)
            .collect()
            .await;
        let mut items: Vec<Candidate> = Vec::new();
        for feed_items in fetched {
            let remaining = RSS_MAX_ENTRIES_PER_RUN - items.len();
            if remaining == 0 {
                break;
            }
            items.extend(feed_items.into_iter().take(remaining));
        }
        items
    }

    /// Licensed Webz.io when configured, with the RSS feeds as the fallback.
    pub async fn fetch_news_items(&self, now: DateTime<Utc>) -> Vec<Candidate> {
        if let Some(webz) = &self.webz {
            match self.fetch_webz(webz, now).await {
                Some(items) if !items.is_empty() => return items,
                Some(_) => tracing::info!(reason = "empty", "news_feed.fallback"),
                None => tracing::info!(reason = "unavailable", "news_feed.fallback"),
            }
        } else {
            tracing::info!("news_feed.webz_unconfigured");
        }
        self.fetch_all_feeds(now).await
    }
}

// ---------------------------------------------------------------------------
// Headline labels
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Categorization {
    pub category: String,
    pub relevance: i32,
    pub summary: Option<String>,
}

/// The deterministic label used when no model answer is available.
pub fn local_categorize_headline(title: &str) -> Categorization {
    const KEYWORD_CATEGORIES: [(&str, &[&str]); 6] = [
        (
            "regulation",
            &[
                "imo",
                "regulation",
                "sanction",
                "compliance",
                "mandate",
                "law",
            ],
        ),
        (
            "carbon",
            &["carbon", "emission", "ets", "fuel eu", "decarbon"],
        ),
        (
            "bunkers",
            &["bunker", "marine fuel", "lng", "methanol", "biofuel"],
        ),
        (
            "commodities",
            &["commodity", "commodities", "crude", "oil price", "energy"],
        ),
        (
            "shipping",
            &["shipping", "vessel", "tanker", "freight", "port"],
        ),
        ("markets", &["market", "trading", "trade", "price"]),
    ];
    let lowered = icu_casemap::CaseMapper::new()
        .fold_string(title)
        .into_owned();
    let category = KEYWORD_CATEGORIES
        .iter()
        .find(|(_, keywords)| keywords.iter().any(|keyword| lowered.contains(keyword)))
        .map_or("markets", |(category, _)| *category);
    Categorization {
        category: category.to_owned(),
        relevance: 3,
        summary: None,
    }
}

/// Find a JSON value in a model answer: the whole text (minus a Markdown
/// fence) first, then the first `{...}` block that has no inner `}`.
pub fn extract_json(text: &str) -> Option<Value> {
    let mut text = text.trim();
    if let Some(rest) = text.strip_prefix("```") {
        text = rest.strip_prefix("json").unwrap_or(rest).trim_start();
        if let Some(inner) = text.trim_end().strip_suffix("```") {
            text = inner.trim_end();
        }
    }
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    let mut start = 0;
    while let Some(open) = text[start..].find('{').map(|offset| start + offset) {
        if let Some(close) = text[open + 1..].find('}').map(|offset| open + 1 + offset) {
            if close > open + 1 {
                return serde_json::from_str(&text[open..=close]).ok();
            }
        } else {
            return None;
        }
        start = open + 1;
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelError {
    /// The key or its permissions were refused: stop calling for this run.
    Auth,
    Other,
}

/// One text completion for one prompt.
pub trait TextModel: Send + Sync {
    fn generate(
        &self,
        prompt: &str,
        timeout: Duration,
    ) -> impl Future<Output = Result<String, ModelError>> + Send;
}

pub struct GeminiClient {
    client: reqwest::Client,
    key: Secret,
    base_url: String,
}

impl GeminiClient {
    pub fn new(key: Secret, base_url: &str) -> Result<Self, Exit> {
        reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .map(|client| Self {
                client,
                key,
                base_url: base_url.trim_end_matches('/').to_owned(),
            })
            .map_err(|_| Exit::Dependency)
    }

    async fn attempt(&self, prompt: &str, timeout: Duration) -> Result<String, AttemptError> {
        let url = format!(
            "{}/v1beta/models/{GEMINI_MODEL}:generateContent",
            self.base_url
        );
        let body = serde_json::json!({"contents": [{"parts": [{"text": prompt}]}]});
        let mut response = self
            .client
            .post(url)
            .header("x-goog-api-key", self.key.expose())
            .timeout(timeout)
            .json(&body)
            .send()
            .await
            .map_err(|_| AttemptError::Retryable)?;
        let status = response.status();
        let mut received = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AttemptError::Retryable)?
        {
            if received.len() + chunk.len() > GEMINI_RESPONSE_MAX_BYTES {
                return Err(AttemptError::Final);
            }
            received.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            if is_auth_failure(status.as_u16(), &received) {
                return Err(AttemptError::Auth);
            }
            return Err(
                if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                    AttemptError::Retryable
                } else {
                    AttemptError::Final
                },
            );
        }
        let payload: Value = serde_json::from_slice(&received).map_err(|_| AttemptError::Final)?;
        let parts = payload
            .pointer("/candidates/0/content/parts")
            .and_then(Value::as_array)
            .ok_or(AttemptError::Final)?;
        let text: String = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect();
        if text.is_empty() {
            Err(AttemptError::Final)
        } else {
            Ok(text)
        }
    }
}

enum AttemptError {
    Auth,
    Retryable,
    Final,
}

/// Status 401 or 403, or one of the key-refusal phrases in the error. The
/// answer is read only here; it is never logged.
fn is_auth_failure(status: u16, body: &[u8]) -> bool {
    if matches!(status, 401 | 403) {
        return true;
    }
    let Ok(payload) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let field = |name: &str| {
        payload
            .pointer(&format!("/error/{name}"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .replace('_', " ")
            .to_lowercase()
    };
    let text = format!("{} {}", field("status"), field("message"));
    [
        "permission denied",
        "unauthorized",
        "unauthenticated",
        "invalid api key",
        "api key was reported as leaked",
        "reported as leaked",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

impl TextModel for GeminiClient {
    /// Two attempts inside the caller's total budget, as the provider SDK did:
    /// each attempt gets half of what is left after the retry delay.
    async fn generate(&self, prompt: &str, timeout: Duration) -> Result<String, ModelError> {
        let attempt_timeout = timeout
            .checked_sub(GEMINI_RETRY_DELAY)
            .map(|budget| budget / GEMINI_ATTEMPTS)
            .filter(|attempt| !attempt.is_zero())
            .ok_or(ModelError::Other)?;
        for attempt in 1..=GEMINI_ATTEMPTS {
            match self.attempt(prompt, attempt_timeout).await {
                Ok(text) => return Ok(text),
                Err(AttemptError::Auth) => return Err(ModelError::Auth),
                Err(AttemptError::Retryable) if attempt < GEMINI_ATTEMPTS => {
                    tokio::time::sleep(GEMINI_RETRY_DELAY).await;
                }
                Err(_) => return Err(ModelError::Other),
            }
        }
        Err(ModelError::Other)
    }
}

/// A failure threshold with a bounded cooldown, for one run.
#[derive(Debug)]
struct Breaker {
    failures: u32,
    opened_until: Option<tokio::time::Instant>,
}

impl Breaker {
    fn new() -> Self {
        Self {
            failures: 0,
            opened_until: None,
        }
    }

    fn allow(&self) -> bool {
        self.opened_until
            .is_none_or(|until| tokio::time::Instant::now() >= until)
    }

    fn success(&mut self) {
        self.failures = 0;
        self.opened_until = None;
    }

    fn failure(&mut self) {
        self.failures += 1;
        if self.failures >= BREAKER_THRESHOLD {
            self.opened_until = Some(tokio::time::Instant::now() + BREAKER_COOLDOWN);
        }
    }
}

/// Labels headlines with the model while it works, and locally otherwise.
///
/// The state is per run: a call budget, a circuit breaker and a switch that
/// turns the model off after the first refused key.
pub struct Categorizer<M> {
    model: Option<M>,
    provider_enabled: bool,
    calls_remaining: u32,
    breaker: Breaker,
    noted: HashSet<&'static str>,
}

impl<M: TextModel> Categorizer<M> {
    pub fn new(model: Option<M>) -> Self {
        Self {
            provider_enabled: model.is_some(),
            model,
            calls_remaining: NEWS_MAX_PROVIDER_CALLS_PER_RUN,
            breaker: Breaker::new(),
            noted: HashSet::new(),
        }
    }

    pub fn provider_enabled(&self) -> bool {
        self.provider_enabled
    }

    fn note(&mut self, reason: &'static str) {
        if self.noted.insert(reason) {
            tracing::warn!(reason, "news_feed.provider_unavailable");
        }
    }

    pub async fn categorize(&mut self, title: &str) -> Categorization {
        let local = || local_categorize_headline(title);
        if self.model.is_none() {
            if self.noted.insert("configuration") {
                tracing::info!(reason = "configuration", "news_feed.provider_unavailable");
            }
            return local();
        }
        if !self.provider_enabled {
            return local();
        }
        if self.calls_remaining == 0 {
            if self.noted.insert("run_call_cap") {
                tracing::info!(reason = "run_call_cap", "news_feed.provider_unavailable");
            }
            return local();
        }
        self.calls_remaining -= 1;
        if !self.breaker.allow() {
            return local();
        }
        let model = self.model.as_ref().expect("model presence checked above");
        let answer = tokio::time::timeout(
            NEWS_PROVIDER_TIMEOUT,
            model.generate(&headline_prompt(title), NEWS_PROVIDER_TIMEOUT),
        )
        .await;
        match answer {
            Ok(Ok(text)) => {
                self.breaker.success();
                match interpret_answer(&text) {
                    Answer::Label(label) => label,
                    Answer::Unusable => local(),
                    Answer::Malformed => {
                        self.breaker.failure();
                        self.note("provider_error");
                        local()
                    }
                }
            }
            Ok(Err(ModelError::Auth)) => {
                self.breaker.failure();
                self.provider_enabled = false;
                if self.noted.insert("auth_or_permission") {
                    tracing::warn!(reason = "auth_or_permission", "news_feed.provider_disabled");
                }
                local()
            }
            Ok(Err(ModelError::Other)) | Err(_) => {
                self.breaker.failure();
                self.note("provider_error");
                local()
            }
        }
    }
}

fn headline_prompt(title: &str) -> String {
    format!(
        "Categorize this shipping/maritime headline into exactly ONE category. \
Pick the single best category from this list: shipping, bunkers, regulation, carbon, commodities, markets. \
Return ONLY a JSON object with these three fields:\n\
- category: one of shipping, bunkers, regulation, carbon, commodities, markets\n\
- relevance: integer 1 to 5 where 5 means highly relevant to marine fuel trading\n\
- summary: a one-line summary if the title is unclear, otherwise null\n\
Do NOT wrap in markdown. Do NOT include explanation.\n\
Headline: {title}"
    )
}

enum Answer {
    Label(Categorization),
    /// No JSON, or an empty JSON value: use the local label, no failure.
    Unusable,
    /// JSON that is not an object: a provider failure, local label.
    Malformed,
}

fn interpret_answer(text: &str) -> Answer {
    let Some(value) = extract_json(text).filter(is_truthy) else {
        return Answer::Unusable;
    };
    let Some(fields) = value.as_object() else {
        return Answer::Malformed;
    };
    let category = fields
        .get("category")
        .and_then(Value::as_str)
        .filter(|category| VALID_CATEGORIES.contains(category))
        .unwrap_or("markets");
    let relevance = fields
        .get("relevance")
        .and_then(Value::as_i64)
        .filter(|relevance| (1..=5).contains(relevance))
        .unwrap_or(3);
    // The same plain-text rule as a Webz summary: no control characters,
    // collapsed whitespace, non-empty, at most NEWS_SUMMARY_MAX_CHARS.
    let summary = fields
        .get("summary")
        .and_then(|summary| bounded_plain_text(summary, NEWS_SUMMARY_MAX_CHARS, true));
    Answer::Label(Categorization {
        category: category.to_owned(),
        relevance: relevance as i32,
        summary,
    })
}

// ---------------------------------------------------------------------------
// The refresh
// ---------------------------------------------------------------------------

/// The database the release uses, for the same-connection target check.
#[derive(Debug, Clone, Copy)]
pub struct RefreshTarget<'a> {
    pub environment: Environment,
    pub database: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// Another process holds the advisory lock. Nothing was fetched.
    LockBusy,
    Completed {
        /// Candidates left after duplicate removal.
        fetched: usize,
        /// Candidates whose URL was not stored when the run looked.
        new: usize,
        /// Rows the insert actually added.
        inserted: u64,
    },
}

struct NewsRow {
    id: Uuid,
    title: String,
    summary: Option<String>,
    source: String,
    source_url: String,
    url: String,
    category: String,
    relevance: i32,
    published_at: DateTime<Utc>,
    fetched_at: DateTime<Utc>,
}

/// Run one refresh in one transaction.
///
/// The advisory lock lives as long as the transaction, so the fetch, the
/// labels and the insert are one exclusive unit. A database failure ends the
/// run with [`Exit::Dependency`]; provider failures never do.
pub async fn refresh_news<F: Fetch, M: TextModel>(
    pool: &PgPool,
    target: RefreshTarget<'_>,
    sources: &NewsSources<F>,
    categorizer: &mut Categorizer<M>,
    now: impl Fn() -> DateTime<Utc>,
) -> Result<Refresh, Exit> {
    let mut transaction = pool.begin().await.map_err(|_| Exit::Dependency)?;
    require_target(&mut transaction, target.environment, target.database).await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(NEWS_REFRESH_ADVISORY_LOCK_ID)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| Exit::Dependency)?;
    if !acquired {
        transaction.rollback().await.map_err(|_| Exit::Dependency)?;
        return Ok(Refresh::LockBusy);
    }

    let candidates = sources.fetch_news_items(now()).await;
    if candidates.is_empty() {
        tracing::info!("news_feed.no_items_fetched");
        transaction.commit().await.map_err(|_| Exit::Dependency)?;
        return Ok(Refresh::Completed {
            fetched: 0,
            new: 0,
            inserted: 0,
        });
    }
    let candidates = dedupe_candidates(candidates);
    let fetched = candidates.len();

    let stored = stored_urls(&mut transaction, &candidates).await?;
    let new_items: Vec<Candidate> = candidates
        .into_iter()
        .filter(|candidate| !stored.contains(&candidate.url))
        .collect();
    if new_items.is_empty() {
        tracing::info!(total = fetched, "news_feed.all_duplicates");
        transaction.commit().await.map_err(|_| Exit::Dependency)?;
        return Ok(Refresh::Completed {
            fetched,
            new: 0,
            inserted: 0,
        });
    }
    tracing::info!(
        new_count = new_items.len(),
        total_fetched = fetched,
        provider_enabled = categorizer.provider_enabled(),
        "news_feed.categorizing"
    );

    let new = new_items.len();
    let mut rows = Vec::with_capacity(new);
    for item in new_items {
        let label = categorizer.categorize(&item.title).await;
        rows.push(NewsRow {
            id: Uuid::new_v4(),
            title: item.title,
            summary: label.summary.or(item.summary),
            source: item.source,
            source_url: item.source_url,
            url: item.url,
            category: label.category,
            relevance: label.relevance,
            published_at: item.published_at,
            fetched_at: now(),
        });
    }
    let inserted = insert_rows(&mut transaction, &rows).await?;
    transaction.commit().await.map_err(|_| Exit::Dependency)?;
    tracing::info!(inserted, "news_feed.refresh_complete");
    Ok(Refresh::Completed {
        fetched,
        new,
        inserted,
    })
}

async fn stored_urls(
    connection: &mut PgConnection,
    candidates: &[Candidate],
) -> Result<HashSet<String>, Exit> {
    let urls: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.url.as_str())
        .collect();
    let stored: Vec<String> = sqlx::query_scalar("SELECT url FROM news_items WHERE url = ANY($1)")
        .bind(urls)
        .fetch_all(&mut *connection)
        .await
        .map_err(|_| Exit::Dependency)?;
    Ok(stored.into_iter().collect())
}

/// Insert the rows and count only those the table accepted. A URL another
/// writer stored after the check is skipped by the conflict clause.
async fn insert_rows(connection: &mut PgConnection, rows: &[NewsRow]) -> Result<u64, Exit> {
    let column =
        |extract: fn(&NewsRow) -> String| -> Vec<String> { rows.iter().map(extract).collect() };
    let result = sqlx::query(
        "INSERT INTO news_items \
         (id, title, summary, source, source_url, url, category, relevance, published_at, fetched_at) \
         SELECT * FROM UNNEST($1::uuid[], $2::text[], $3::text[], $4::text[], $5::text[], \
                              $6::text[], $7::text[], $8::int4[], $9::timestamptz[], $10::timestamptz[]) \
         ON CONFLICT (url) DO NOTHING",
    )
    .bind(rows.iter().map(|row| row.id).collect::<Vec<_>>())
    .bind(column(|row| row.title.clone()))
    .bind(rows.iter().map(|row| row.summary.clone()).collect::<Vec<_>>())
    .bind(column(|row| row.source.clone()))
    .bind(column(|row| row.source_url.clone()))
    .bind(column(|row| row.url.clone()))
    .bind(column(|row| row.category.clone()))
    .bind(rows.iter().map(|row| row.relevance).collect::<Vec<_>>())
    .bind(rows.iter().map(|row| row.published_at).collect::<Vec<_>>())
    .bind(rows.iter().map(|row| row.fetched_at).collect::<Vec<_>>())
    .execute(&mut *connection)
    .await
    .map_err(|_| Exit::Dependency)?;
    Ok(result.rows_affected())
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// Every refusal that needs no database or network, in this order: release
/// and identity, deploy state, `--apply`, settings. The deployed environment
/// and release come from the job environment (`ENVIRONMENT`, `RELEASE_SHA`).
pub fn preflight(
    args: &JobArgs,
    vars: &HashMap<String, String>,
    deploy_state: &Path,
) -> Result<NewsConfig, Exit> {
    let expected_sha = &args.identity.release_sha;
    let full_sha = expected_sha.len() == 40
        && expected_sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !full_sha || !deploy_state.is_absolute() {
        return Err(Exit::Precondition);
    }
    let value = |key: &str| vars.get(key).map(String::as_str).unwrap_or("");
    require_job_preflight(
        value("ENVIRONMENT"),
        value("RELEASE_SHA"),
        args.identity.environment,
        expected_sha,
        deploy_state,
    )?;
    require_apply(args.apply)?;
    NewsConfig::from_environment(vars)
}

/// The deploy state path of the environment: `<state dir>/<environment>.state`.
pub fn deploy_state_path(
    vars: &HashMap<String, String>,
    environment: Environment,
) -> Result<PathBuf, Exit> {
    let directory = PathBuf::from(vars.get("DEPLOY_STATE_DIR").ok_or(Exit::Precondition)?);
    if !directory.is_absolute() {
        return Err(Exit::Precondition);
    }
    Ok(directory.join(format!("{}.state", environment.as_str())))
}

/// `verdaxis-read job news` with its environment, deploy-state path and pool
/// opener supplied by the caller.
pub async fn run_with<O, Fut>(
    args: &JobArgs,
    vars: &HashMap<String, String>,
    deploy_state: &Path,
    open: O,
) -> Exit
where
    O: FnOnce(Environment, String) -> Fut,
    Fut: Future<Output = Result<RunContext, Exit>>,
{
    let (exit, outcome, counts) = match execute(args, vars, deploy_state, open).await {
        Ok(Refresh::LockBusy) => (Exit::Ok, "lock_busy", serde_json::json!({"inserted": 0})),
        Ok(Refresh::Completed {
            fetched,
            new,
            inserted,
        }) => (
            Exit::Ok,
            "complete",
            serde_json::json!({"fetched": fetched, "new": new, "inserted": inserted}),
        ),
        Err(exit) => (exit, "refused", serde_json::json!({})),
    };
    eprintln!(
        "{}",
        serde_json::json!({"command": "job news", "outcome": outcome, "counts": counts})
    );
    exit
}

/// The process environment as text. A variable that is not valid Unicode is
/// left out instead of aborting the process.
pub fn environment_from_os(
    variables: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> HashMap<String, String> {
    variables
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// Entry point for `job news`. The owner of `cli/mod.rs` routes
/// `JobKind::News` here.
pub async fn run(args: &JobArgs) -> Exit {
    let _ = tracing_subscriber::fmt()
        .json()
        .with_writer(std::io::stderr)
        .try_init();
    let vars = environment_from_os(std::env::vars_os());
    let Ok(state) = deploy_state_path(&vars, args.identity.environment) else {
        return Exit::Precondition;
    };
    run_with(args, &vars, &state, |environment, release_sha| async move {
        context::open(Purpose::Job(JobKind::News), environment, &release_sha).await
    })
    .await
}

async fn execute<O, Fut>(
    args: &JobArgs,
    vars: &HashMap<String, String>,
    deploy_state: &Path,
    open: O,
) -> Result<Refresh, Exit>
where
    O: FnOnce(Environment, String) -> Fut,
    Fut: Future<Output = Result<RunContext, Exit>>,
{
    let config = preflight(args, vars, deploy_state)?;
    let context = open(args.identity.environment, args.identity.release_sha.clone()).await?;
    let database = context
        .pool
        .connect_options()
        .get_database()
        .ok_or(Exit::Precondition)?
        .to_owned();
    let sources = NewsSources {
        fetch: ReqwestFetch::new()?,
        webz: config.webz,
        feeds: default_feeds(),
    };
    let model = config
        .gemini_api_key
        .map(|key| GeminiClient::new(key, GEMINI_BASE_URL))
        .transpose()?;
    let mut categorizer = Categorizer::new(model);
    refresh_news(
        &context.pool,
        RefreshTarget {
            environment: args.identity.environment,
            database: &database,
        },
        &sources,
        &mut categorizer,
        Utc::now,
    )
    .await
}
