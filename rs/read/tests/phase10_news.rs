//! Phase 10 Task 11: the news refresh job, offline cases.
//!
//! Provider traffic runs against in-memory fakes, or against a loopback server
//! where the bounded reader itself is the subject. No database is used here;
//! `phase10_news_postgres.rs` covers the lock and the insert.
use axum::{
    Router,
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use verdaxis_read::cli::Exit;
use verdaxis_read::cli::jobs::news::*;
use verdaxis_read::cli::{Environment, IdentityArgs, JobArgs, JobKind};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap()
}

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

/// The URL and query pairs of one request.
type RecordedCall = (String, Vec<(String, String)>);

#[derive(Default)]
struct FakeFetch {
    responses: Mutex<HashMap<String, Result<Vec<u8>, FetchError>>>,
    calls: Mutex<Vec<RecordedCall>>,
}

impl FakeFetch {
    fn with(responses: Vec<(&str, Result<Vec<u8>, FetchError>)>) -> Self {
        Self {
            responses: Mutex::new(
                responses
                    .into_iter()
                    .map(|(url, response)| (url.to_owned(), response))
                    .collect(),
            ),
            calls: Mutex::default(),
        }
    }

    fn urls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(url, _)| url.clone())
            .collect()
    }
}

impl Fetch for FakeFetch {
    async fn get(&self, request: FetchRequest<'_>) -> Result<Vec<u8>, FetchError> {
        self.calls.lock().unwrap().push((
            request.url.to_owned(),
            request
                .query
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        ));
        self.responses
            .lock()
            .unwrap()
            .get(request.url)
            .cloned()
            .unwrap_or(Err(FetchError::Unavailable))
    }
}

/// A model that replays queued answers and counts its calls.
struct FakeModel {
    answers: Mutex<VecDeque<Result<String, ModelError>>>,
    fallback: Result<String, ModelError>,
    calls: Mutex<u32>,
}

impl FakeModel {
    fn always(answer: Result<String, ModelError>) -> Self {
        Self {
            answers: Mutex::default(),
            fallback: answer,
            calls: Mutex::new(0),
        }
    }

    fn calls(&self) -> u32 {
        *self.calls.lock().unwrap()
    }
}

impl TextModel for &FakeModel {
    async fn generate(&self, _prompt: &str, _timeout: Duration) -> Result<String, ModelError> {
        *self.calls.lock().unwrap() += 1;
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| self.fallback.clone())
    }
}

fn rss(items: &[(&str, &str)]) -> Vec<u8> {
    let body: String = items
        .iter()
        .map(|(title, link)| {
            format!(
                "<item><title>{title}</title><link>{link}</link>\
                 <pubDate>Fri, 30 Oct 2026 08:00:00 GMT</pubDate></item>"
            )
        })
        .collect();
    format!(
        "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Feed</title>\
         <link>https://example.test</link><description>d</description>{body}</channel></rss>"
    )
    .into_bytes()
}

fn webz_body(posts: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "posts": posts })).unwrap()
}

fn feed(name: &str, url: &str, domain: &str) -> FeedSpec {
    FeedSpec {
        name: name.into(),
        url: url.into(),
        article_domains: vec![domain.into()],
    }
}

fn webz_config() -> WebzConfig {
    WebzConfig {
        token: Secret::new("webz-fixture-token"),
        url: WEBZ_DEFAULT_URL.into(),
        timeout: Duration::from_secs(15),
    }
}

// ---------------------------------------------------------------------------
// Source order: Webz first, RSS as the fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn news_falls_back_to_rss_when_webz_is_absent() {
    let fetch = FakeFetch::with(vec![(
        "https://feed.test/rss",
        Ok(rss(&[("Methanol bunkering grows", "https://feed.test/a")])),
    )]);
    let sources = NewsSources {
        fetch,
        webz: None,
        feeds: vec![feed("Feed", "https://feed.test/rss", "feed.test")],
    };
    let items = sources.fetch_news_items(now()).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].source, "Feed");
    assert_eq!(items[0].source_url, "https://feed.test/rss");
    assert_eq!(
        sources.fetch.urls(),
        vec!["https://feed.test/rss"],
        "no Webz request without a token"
    );
}

#[tokio::test]
async fn webz_items_win_and_rss_is_not_read() {
    let webz = webz_config();
    let fetch = FakeFetch::with(vec![(
        WEBZ_DEFAULT_URL,
        Ok(webz_body(json!([{
            "title": "Bio-methanol shipping order",
            "url": "https://news.example.org/a",
            "thread": {"site": "News Example"},
            "published": "2026-10-31T10:00:00Z",
            "summary": "A summary."
        }]))),
    )]);
    let sources = NewsSources {
        fetch,
        webz: Some(webz),
        feeds: vec![feed("Feed", "https://feed.test/rss", "feed.test")],
    };
    let items = sources.fetch_news_items(now()).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].source, "News Example");
    assert_eq!(items[0].source_url, "https://news.example.org");
    assert_eq!(items[0].summary.as_deref(), Some("A summary."));
    assert_eq!(sources.fetch.urls(), vec![WEBZ_DEFAULT_URL]);

    // The one request carries the documented parameters and the query text.
    let calls = sources.fetch.calls.lock().unwrap();
    let query: HashMap<&str, &str> = calls[0]
        .1
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    assert_eq!(query["token"], "webz-fixture-token");
    assert_eq!(query["format"], "json");
    assert_eq!(query["sort"], "published");
    assert_eq!(query["order"], "desc");
    assert_eq!(query["size"], "100");
    assert_eq!(query["includeSyndicated"], "false");
    assert_eq!(query["q"], webz_query());
    assert!(query["q"].starts_with("site_type:news AND (((title:(\"bio methanol\" OR "));
    assert!(query["q"].ends_with("\"marine fuel regulation\"))"));
}

#[tokio::test]
async fn empty_or_unavailable_webz_falls_back_to_rss() {
    for webz_answer in [
        Ok(webz_body(json!([]))),
        Ok(webz_body(
            json!([{"title": "", "url": "https://x.example.org/a"}]),
        )),
        Ok(b"not json".to_vec()),
        Ok(serde_json::to_vec(&json!({"posts": "nope"})).unwrap()),
        Err(FetchError::Unavailable),
        Err(FetchError::TooLarge),
    ] {
        let fetch = FakeFetch::with(vec![
            (WEBZ_DEFAULT_URL, webz_answer),
            (
                "https://feed.test/rss",
                Ok(rss(&[("Port news", "https://feed.test/p")])),
            ),
        ]);
        let sources = NewsSources {
            fetch,
            webz: Some(webz_config()),
            feeds: vec![feed("Feed", "https://feed.test/rss", "feed.test")],
        };
        let items = sources.fetch_news_items(now()).await;
        assert_eq!(items.len(), 1, "the RSS item is used");
        assert_eq!(
            sources.fetch.urls(),
            vec![WEBZ_DEFAULT_URL, "https://feed.test/rss"]
        );
    }
}

#[tokio::test]
async fn feeds_keep_order_and_the_per_run_cap() {
    let many = |prefix: &str, count: usize| -> Vec<u8> {
        let owned: Vec<(String, String)> = (0..count)
            .map(|index| {
                (
                    format!("{prefix} headline {index}"),
                    format!("https://{prefix}.test/{index}"),
                )
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = owned
            .iter()
            .map(|(title, link)| (title.as_str(), link.as_str()))
            .collect();
        rss(&borrowed)
    };
    let fetch = FakeFetch::with(vec![
        ("https://a.test/rss", Ok(many("a", 150))),
        ("https://b.test/rss", Ok(many("b", 150))),
        ("https://c.test/rss", Err(FetchError::Unavailable)),
        (
            "https://d.test/rss",
            Ok(b"<html>not a feed</html>".to_vec()),
        ),
        ("https://e.test/rss", Ok(many("e", 150))),
    ]);
    let sources = NewsSources {
        fetch,
        webz: None,
        feeds: vec![
            feed("A", "https://a.test/rss", "a.test"),
            feed("B", "https://b.test/rss", "b.test"),
            feed("C", "https://c.test/rss", "c.test"),
            feed("D", "https://d.test/rss", "d.test"),
            feed("E", "https://e.test/rss", "e.test"),
        ],
    };
    let items = sources.fetch_news_items(now()).await;
    // 100 entries are read from each feed, then the run keeps 250 in order.
    assert_eq!(items.len(), 250);
    assert_eq!(items.iter().filter(|item| item.source == "A").count(), 100);
    assert_eq!(items.iter().filter(|item| item.source == "B").count(), 100);
    assert_eq!(items.iter().filter(|item| item.source == "E").count(), 50);
    assert_eq!(items[0].source, "A");
    assert_eq!(items[100].source, "B");
}

// ---------------------------------------------------------------------------
// Bounded readers: a loopback server drives the real client
// ---------------------------------------------------------------------------

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{address}")
}

fn sized(bytes: usize) -> Response {
    // Streamed in 64 KiB chunks, so the reader meets the cap mid-body.
    let chunk = vec![b'x'; 64 * 1024];
    let mut remaining = bytes;
    let stream = futures_util::stream::iter(std::iter::from_fn(move || {
        if remaining == 0 {
            return None;
        }
        let take = remaining.min(chunk.len());
        remaining -= take;
        Some(Ok::<_, std::io::Error>(chunk[..take].to_vec()))
    }));
    Response::new(Body::from_stream(stream))
}

#[tokio::test]
async fn news_rejects_oversize_provider_body_and_feed() {
    let router = Router::new()
        .route(
            "/webz-at-cap",
            get(|| async { sized(WEBZ_MAX_RESPONSE_BYTES) }),
        )
        .route(
            "/webz-over",
            get(|| async { sized(WEBZ_MAX_RESPONSE_BYTES + 1) }),
        )
        .route(
            "/feed-over",
            get(|| async { sized(RSS_MAX_BYTES_PER_FEED + 1) }),
        )
        .route("/missing", get(|| async { StatusCode::NOT_FOUND }))
        .route(
            "/moved",
            get(|| async {
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header("location", "/webz-at-cap")
                    .body(Body::empty())
                    .unwrap()
            }),
        );
    let base = serve(router).await;
    let fetch = ReqwestFetch::new().unwrap();
    let read = |path: &str, max_bytes: usize| {
        let url = format!("{base}{path}");
        let fetch = &fetch;
        async move {
            fetch
                .get(FetchRequest {
                    url: &url,
                    query: &[],
                    max_bytes,
                    timeout: Duration::from_secs(10),
                })
                .await
        }
    };

    // A body exactly at the cap is read; one byte more is refused.
    assert_eq!(
        read("/webz-at-cap", WEBZ_MAX_RESPONSE_BYTES)
            .await
            .map(|body| body.len()),
        Ok(WEBZ_MAX_RESPONSE_BYTES)
    );
    assert_eq!(
        read("/webz-over", WEBZ_MAX_RESPONSE_BYTES).await,
        Err(FetchError::TooLarge)
    );
    assert_eq!(
        read("/feed-over", RSS_MAX_BYTES_PER_FEED).await,
        Err(FetchError::TooLarge)
    );
    // Non-2xx answers and redirects are failures, not bodies.
    assert_eq!(read("/missing", 1024).await, Err(FetchError::Unavailable));
    assert_eq!(
        read("/moved", WEBZ_MAX_RESPONSE_BYTES).await,
        Err(FetchError::Unavailable)
    );

    // An oversize Webz body falls back to the feeds; an oversize feed adds no item.
    let sources = NewsSources {
        fetch,
        webz: Some(WebzConfig {
            url: format!("{base}/webz-over"),
            ..webz_config()
        }),
        feeds: vec![feed("Big", &format!("{base}/feed-over"), "feed.test")],
    };
    assert!(
        sources
            .fetch_webz(sources.webz.as_ref().unwrap(), now())
            .await
            .is_none()
    );
    assert!(sources.fetch_news_items(now()).await.is_empty());
}

#[tokio::test]
async fn webz_token_is_sent_as_a_query_parameter_and_never_printed() {
    let router = Router::new().route(
        "/filter",
        get(
            |axum::extract::RawQuery(query): axum::extract::RawQuery| async move {
                let query = query.unwrap_or_default();
                let ok = query.contains("token=webz-fixture-token")
                    && query.contains("format=json")
                    && query.contains("includeSyndicated=false");
                let status = if ok {
                    StatusCode::OK
                } else {
                    StatusCode::BAD_REQUEST
                };
                Response::builder()
                    .status(status)
                    .body(Body::from(
                        serde_json::to_vec(&json!({"posts": []})).unwrap(),
                    ))
                    .unwrap()
            },
        ),
    );
    let base = serve(router).await;
    let sources = NewsSources {
        fetch: ReqwestFetch::new().unwrap(),
        webz: None,
        feeds: vec![],
    };
    let config = WebzConfig {
        url: format!("{base}/filter"),
        ..webz_config()
    };
    assert_eq!(sources.fetch_webz(&config, now()).await, Some(vec![]));
    assert!(!format!("{config:?}").contains("webz-fixture-token"));
    assert!(!format!("{:?}", Secret::new("another-fixture")).contains("another-fixture"));
}

// ---------------------------------------------------------------------------
// URL, feed and Webz-post validation
// ---------------------------------------------------------------------------

#[test]
fn public_urls_are_accepted_and_private_or_malformed_ones_refused() {
    for good in [
        "https://example.org/a",
        "http://example.org:8080/a?b=c#d",
        "  https://example.org/padded  ",
        "https://8.8.8.8/x",
        "https://[2606:4700:4700::1111]/x",
    ] {
        assert_eq!(
            validated_public_http_url(good, NEWS_URL_MAX_CHARS).as_deref(),
            Some(good.trim()),
            "{good}"
        );
    }
    for bad in [
        "",
        "   ",
        "ftp://example.org/a",
        "javascript:alert(1)",
        "https:example.org/a",
        "https://user@example.org/a",
        "https://user:secret@example.org/a",
        "https://@example.org/a",
        "https://example.org:99999/a",
        "https://exa mple.org/a",
        "https://example.org/a\nb",
        "https://example.org/\u{7f}",
        "https://localhost/a",
        "https://printer.local/a",
        "https://api.internal/a",
        "https://nas.home/a",
        "https://box.lan/a",
        "https://box.localhost/a",
        "https://127.0.0.1/a",
        "https://10.1.2.3/a",
        "https://172.16.0.1/a",
        "https://192.168.1.1/a",
        "https://169.254.169.254/latest",
        "https://100.64.0.1/a",
        "https://0.0.0.0/a",
        "https://224.0.0.1/a",
        "https://0x7f.1/a",
        "https://[::1]/a",
        "https://[fe80::1]/a",
        "https://[fd00::1]/a",
        "https://[::ffff:10.0.0.1]/a",
        "https://[2001:db8::1]/a",
    ] {
        assert_eq!(
            validated_public_http_url(bad, NEWS_URL_MAX_CHARS),
            None,
            "{bad:?}"
        );
    }
    let long = format!("https://example.org/{}", "a".repeat(NEWS_URL_MAX_CHARS));
    assert_eq!(validated_public_http_url(&long, NEWS_URL_MAX_CHARS), None);
    let at_limit = format!(
        "https://example.org/{}",
        "a".repeat(NEWS_URL_MAX_CHARS - "https://example.org/".len())
    );
    assert!(validated_public_http_url(&at_limit, NEWS_URL_MAX_CHARS).is_some());
}

#[test]
fn article_urls_must_belong_to_the_publisher() {
    let domains = vec!["publisher.test".to_owned()];
    for good in [
        "https://publisher.test/a",
        "https://www.publisher.test/a",
        "https://PUBLISHER.test./a",
    ] {
        assert!(validated_article_url(good, &domains).is_some(), "{good}");
    }
    for bad in [
        "https://other.test/a",
        "https://notpublisher.test/a",
        "https://publisher.test.evil.test/a",
        "https://evil.test/publisher.test",
    ] {
        assert!(validated_article_url(bad, &domains).is_none(), "{bad}");
    }
}

#[test]
fn feed_entries_are_bounded_and_publisher_checked() {
    let spec = feed("Pub", "https://publisher.test/rss", "publisher.test");
    let long_title = "T".repeat(NEWS_TITLE_MAX_CHARS + 1);
    let max_title = "T".repeat(NEWS_TITLE_MAX_CHARS);
    let body = rss(&[
        ("Good headline", "https://publisher.test/good"),
        ("Foreign link", "https://elsewhere.test/x"),
        ("Private link", "http://127.0.0.1/x"),
        (long_title.as_str(), "https://publisher.test/long"),
        (max_title.as_str(), "https://publisher.test/max"),
        ("   ", "https://publisher.test/blank"),
    ]);
    let items = parse_feed(&body, &spec, now()).expect("a feed that parses");
    let urls: Vec<&str> = items.iter().map(|item| item.url.as_str()).collect();
    assert_eq!(
        urls,
        vec!["https://publisher.test/good", "https://publisher.test/max"]
    );
    assert_eq!(items[0].source, "Pub");
    assert_eq!(items[0].source_url, "https://publisher.test/rss");
    assert_eq!(
        items[0].published_at,
        Utc.with_ymd_and_hms(2026, 10, 30, 8, 0, 0).unwrap()
    );
    assert!(items[0].summary.is_none());
    assert!(parse_feed(b"plain text", &spec, now()).is_none());
}

#[test]
fn an_entry_without_a_date_is_stamped_with_the_run_time() {
    let spec = feed("Pub", "https://publisher.test/rss", "publisher.test");
    let body = b"<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>f</title>\
        <link>https://publisher.test</link><description>d</description>\
        <item><title>Undated</title><link>https://publisher.test/u</link></item>\
        </channel></rss>";
    let items = parse_feed(body, &spec, now()).unwrap();
    assert_eq!(items[0].published_at, now());
}

#[test]
fn atom_entries_use_the_alternate_link_and_updated_date() {
    let spec = feed("Pub", "https://publisher.test/atom", "publisher.test");
    let body = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\
        <feed xmlns=\"http://www.w3.org/2005/Atom\"><title>t</title><id>urn:x</id>\
        <updated>2026-10-31T00:00:00Z</updated>\
        <entry><title>Atom headline</title><id>urn:e1</id>\
        <link rel=\"self\" href=\"https://publisher.test/self\"/>\
        <link rel=\"alternate\" href=\"https://publisher.test/alt\"/>\
        <updated>2026-10-30T09:30:00Z</updated></entry></feed>";
    let items = parse_feed(body, &spec, now()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].url, "https://publisher.test/alt");
    assert_eq!(
        items[0].published_at,
        Utc.with_ymd_and_hms(2026, 10, 30, 9, 30, 0).unwrap()
    );
}

#[test]
fn webz_posts_are_validated_field_by_field() {
    let accept = |post: Value| validated_webz_post(&post, now());

    // Thread fields stand in for missing post fields; the site falls back to
    // the host name; the summary keeps its collapsed line breaks.
    let item = accept(json!({
        "thread": {
            "title": "Thread title",
            "url": "https://Host.Example.org:8443/path?q=1",
            "published": 1_788_000_000_000_i64
        },
        "summary": "Line one\nline   two"
    }))
    .expect("a post carried by its thread");
    assert_eq!(item.title, "Thread title");
    assert_eq!(item.source, "host.example.org");
    assert_eq!(item.source_url, "https://Host.Example.org:8443");
    assert_eq!(item.summary.as_deref(), Some("Line one line two"));
    assert_eq!(
        item.published_at,
        Utc.timestamp_opt(1_788_000_000, 0).unwrap()
    );

    let item = accept(json!({
        "title": "  Spaced   title ",
        "url": "https://news.example.org/a",
        "thread": {"site": "Site Name", "site_full": "ignored"},
    }))
    .unwrap();
    assert_eq!(item.title, "Spaced title");
    assert_eq!(item.source, "Site Name");
    assert_eq!(item.published_at, now(), "no date means the run time");
    assert!(item.summary.is_none());

    for bad in [
        json!("not an object"),
        json!({"url": "https://news.example.org/a"}),
        json!({"title": "No url"}),
        json!({"title": "Bad\tcontrol", "url": "https://news.example.org/a"}),
        json!({"title": "x".repeat(501), "url": "https://news.example.org/a"}),
        json!({"title": 5, "url": "https://news.example.org/a"}),
        json!({"title": "Private", "url": "http://10.0.0.1/a"}),
        json!({"title": "Site too long", "url": "https://news.example.org/a",
               "thread": {"site": "s".repeat(101)}}),
        json!({"title": "Site with control", "url": "https://news.example.org/a",
               "thread": {"site": "a\u{1}b"}}),
    ] {
        assert!(accept(bad.clone()).is_none(), "{bad}");
    }

    // A malformed or oversize summary only drops the summary.
    let item = accept(json!({
        "title": "Headline", "url": "https://news.example.org/a",
        "summary": "x".repeat(NEWS_SUMMARY_MAX_CHARS + 1)
    }))
    .unwrap();
    assert!(item.summary.is_none());

    // A source URL that cannot fit its column skips the post.
    let host = format!("{}.example.org", "h".repeat(490));
    assert!(
        accept(json!({"title": "Wide", "url": format!("https://{host}/a")})).is_none(),
        "source_url would exceed {NEWS_SOURCE_URL_MAX_CHARS} characters"
    );
}

#[test]
fn webz_published_accepts_epoch_and_iso_values() {
    let at = |value: Value| parse_webz_published(Some(&value), now());
    let expected = Utc.with_ymd_and_hms(2026, 10, 31, 10, 0, 0).unwrap();
    assert_eq!(at(json!(expected.timestamp())), expected);
    assert_eq!(at(json!(expected.timestamp_millis())), expected);
    assert_eq!(
        at(json!(expected.timestamp() as f64 + 0.5)).timestamp(),
        expected.timestamp()
    );
    for text in [
        "2026-10-31T10:00:00Z",
        "2026-10-31T10:00:00+00:00",
        "2026-10-31T12:00:00.000+0200",
        "2026-10-31T13:00:00.000+03:00",
        " 2026-10-31T10:00:00Z ",
    ] {
        assert_eq!(at(json!(text)), expected, "{text}");
    }
    for fallback in [
        json!(-5),
        json!("2026-10-31T10:00:00"),
        json!("yesterday"),
        json!(1e30),
        json!(null),
        json!(true),
        json!([]),
    ] {
        assert_eq!(at(fallback.clone()), now(), "{fallback}");
    }
    assert_eq!(parse_webz_published(None, now()), now());
}

#[test]
fn duplicates_collapse_by_url_and_by_source_and_headline() {
    let candidate = |title: &str, url: &str, source: &str| Candidate {
        title: title.into(),
        url: url.into(),
        source: source.into(),
        source_url: "https://s.test".into(),
        published_at: now(),
        summary: None,
    };
    let kept = dedupe_candidates(vec![
        candidate("Straße news", "https://s.test/1", "Pub"),
        candidate("Other", "https://s.test/1", "Pub"),
        candidate("STRASSE NEWS", "https://s.test/2", "pub"),
        candidate("Straße news", "https://s.test/3", "Another"),
        candidate("Fresh", "https://s.test/4", "Pub"),
    ]);
    let urls: Vec<&str> = kept.iter().map(|item| item.url.as_str()).collect();
    assert_eq!(
        urls,
        vec!["https://s.test/1", "https://s.test/3", "https://s.test/4"]
    );

    let many: Vec<Candidate> = (0..300)
        .map(|index| {
            candidate(
                &format!("t{index}"),
                &format!("https://s.test/{index}"),
                "P",
            )
        })
        .collect();
    assert_eq!(dedupe_candidates(many).len(), RSS_MAX_ENTRIES_PER_RUN);
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

#[test]
fn local_labels_follow_the_keyword_priority() {
    for (title, category) in [
        ("IMO adopts new regulation", "regulation"),
        ("EU ETS carbon price", "carbon"),
        ("Biofuel bunker demand", "bunkers"),
        ("Crude oil price rises", "commodities"),
        ("Tanker freight rates", "shipping"),
        ("Trading volumes", "markets"),
        ("Nothing relevant at all", "markets"),
        ("LNG vessel delivered", "bunkers"),
        ("Port congestion", "shipping"),
    ] {
        let label = local_categorize_headline(title);
        assert_eq!(label.category, category, "{title}");
        assert_eq!(label.relevance, 3);
        assert_eq!(label.summary, None);
    }
}

#[test]
fn model_answers_are_found_in_plain_fenced_and_surrounded_text() {
    assert_eq!(extract_json("{\"a\": 1}"), Some(json!({"a": 1})));
    assert_eq!(
        extract_json("```json\n{\"a\": 1}\n```"),
        Some(json!({"a": 1}))
    );
    assert_eq!(
        extract_json("```\n{\"a\": 1}\n```  "),
        Some(json!({"a": 1}))
    );
    assert_eq!(
        extract_json("Sure! {\"category\": \"carbon\"} Hope that helps."),
        Some(json!({"category": "carbon"}))
    );
    assert_eq!(extract_json("{} then {\"b\": 2}"), Some(json!({"b": 2})));
    assert_eq!(extract_json("no braces"), None);
    assert_eq!(extract_json("{unterminated"), None);
    assert_eq!(extract_json("{not json}"), None);
}

fn answer(category: &str, relevance: Value, summary: Value) -> Result<String, ModelError> {
    Ok(json!({"category": category, "relevance": relevance, "summary": summary}).to_string())
}

#[tokio::test]
async fn a_model_answer_sets_category_relevance_and_summary() {
    let model = FakeModel::always(answer("carbon", json!(5), json!("Short summary")));
    let mut categorizer = Categorizer::new(Some(&model));
    let label = categorizer.categorize("Anything").await;
    assert_eq!(
        label,
        Categorization {
            category: "carbon".into(),
            relevance: 5,
            summary: Some("Short summary".into())
        }
    );
    assert_eq!(model.calls(), 1);
}

#[tokio::test]
async fn invalid_model_fields_fall_back_to_their_defaults() {
    for (response, category, relevance, summary) in [
        (answer("weather", json!(9), json!(7)), "markets", 3, None),
        (
            answer("shipping", json!(0), json!(null)),
            "shipping",
            3,
            None,
        ),
        (
            answer("shipping", json!("4"), json!("")),
            "shipping",
            3,
            None,
        ),
        (
            answer("shipping", json!(4.5), json!("ok")),
            "shipping",
            3,
            Some("ok"),
        ),
        (Ok("{\"relevance\": 2}".into()), "markets", 2, None),
    ] {
        let model = FakeModel::always(response);
        let mut categorizer = Categorizer::new(Some(&model));
        let label = categorizer.categorize("Anything").await;
        assert_eq!(label.category, category);
        assert_eq!(label.relevance, relevance);
        assert_eq!(label.summary.as_deref(), summary);
    }
}

/// A model summary is stored under the same rule as a Webz summary: plain text
/// with no control characters, whitespace collapsed, non-empty and bounded.
#[tokio::test]
async fn model_summaries_are_bounded_plain_text() {
    let summary_of = |summary: Value| async move {
        let model = FakeModel::always(answer("carbon", json!(4), summary));
        let mut categorizer = Categorizer::new(Some(&model));
        categorizer.categorize("Anything").await.summary
    };

    // Markup is not interpreted or removed: it is stored as inert text, exactly
    // as a Webz summary would be.
    assert_eq!(
        summary_of(json!("<script>alert(1)</script> <b>bold</b>"))
            .await
            .as_deref(),
        Some("<script>alert(1)</script> <b>bold</b>")
    );
    // Whitespace collapses and line breaks are allowed.
    assert_eq!(
        summary_of(json!("  Line one\nline   two\t "))
            .await
            .as_deref(),
        Some("Line one line two")
    );
    // The length bound counts characters and is inclusive.
    let at_limit = "s".repeat(NEWS_SUMMARY_MAX_CHARS);
    assert_eq!(summary_of(json!(at_limit.clone())).await, Some(at_limit));
    assert_eq!(
        summary_of(json!("s".repeat(NEWS_SUMMARY_MAX_CHARS + 1))).await,
        None
    );
    assert_eq!(
        summary_of(json!("é".repeat(NEWS_SUMMARY_MAX_CHARS + 1))).await,
        None,
        "characters, not bytes"
    );
    // Control characters other than tab, carriage return and line feed refuse
    // the whole summary.
    for bad in ["a\u{0}b", "a\u{1b}[31mred", "a\u{7f}b", "a\u{8}b"] {
        assert_eq!(summary_of(json!(bad)).await, None, "{bad:?}");
    }
    // Empty, blank and non-text summaries are no summary; the label stays.
    for empty in [
        json!(""),
        json!("   \n "),
        json!(null),
        json!(7),
        json!(["x"]),
    ] {
        assert_eq!(summary_of(empty.clone()).await, None, "{empty}");
    }
    let model = FakeModel::always(answer("carbon", json!(4), json!("x".repeat(5000))));
    let label = Categorizer::new(Some(&model)).categorize("Anything").await;
    assert_eq!((label.category.as_str(), label.relevance), ("carbon", 4));
}

#[tokio::test]
async fn unusable_model_text_uses_the_local_label() {
    for text in ["no json here", "{}", "null", "[]", "[1, 2]", "42"] {
        let model = FakeModel::always(Ok(text.into()));
        let mut categorizer = Categorizer::new(Some(&model));
        let label = categorizer.categorize("Carbon emissions").await;
        assert_eq!(label.category, "carbon", "{text}");
        assert_eq!(model.calls(), 1);
    }
}

#[tokio::test]
async fn no_key_means_no_provider_call() {
    let mut categorizer = Categorizer::<&FakeModel>::new(None);
    assert!(!categorizer.provider_enabled());
    assert_eq!(
        categorizer.categorize("IMO regulation").await.category,
        "regulation"
    );
}

#[tokio::test]
async fn a_refused_key_turns_the_provider_off_for_the_run() {
    let model = FakeModel::always(Err(ModelError::Auth));
    let mut categorizer = Categorizer::new(Some(&model));
    for _ in 0..5 {
        categorizer.categorize("Port news").await;
    }
    assert_eq!(model.calls(), 1, "no call after the first refusal");
    assert!(!categorizer.provider_enabled());
}

#[tokio::test(start_paused = true)]
async fn three_provider_failures_open_the_breaker_for_a_minute() {
    let model = FakeModel::always(Err(ModelError::Other));
    let mut categorizer = Categorizer::new(Some(&model));
    for _ in 0..6 {
        let label = categorizer.categorize("LNG bunker").await;
        assert_eq!(label.category, "bunkers", "local label while failing");
    }
    assert_eq!(model.calls(), 3, "the breaker opens after three failures");
    tokio::time::advance(Duration::from_secs(61)).await;
    categorizer.categorize("LNG bunker").await;
    assert_eq!(
        model.calls(),
        4,
        "the breaker allows a call after its cooldown"
    );
}

#[tokio::test]
async fn one_run_makes_at_most_fifty_provider_calls() {
    let model = FakeModel::always(answer("carbon", json!(4), json!(null)));
    let mut categorizer = Categorizer::new(Some(&model));
    let mut modelled = 0;
    for index in 0..60 {
        let label = categorizer.categorize(&format!("Headline {index}")).await;
        if label.relevance == 4 {
            modelled += 1;
        }
    }
    assert_eq!(model.calls(), NEWS_MAX_PROVIDER_CALLS_PER_RUN);
    assert_eq!(modelled, 50);
}

// ---------------------------------------------------------------------------
// Gemini client against a loopback server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_gemini_client_sends_the_key_in_a_header_and_reads_the_first_candidate() {
    let router = Router::new().route(
        "/v1beta/models/gemini-2.5-flash-lite:generateContent",
        axum::routing::post(
            |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| async move {
                let key_ok = headers
                    .get("x-goog-api-key")
                    .is_some_and(|value| value == "fixture-key");
                let prompt = body["contents"][0]["parts"][0]["text"]
                    .as_str()
                    .unwrap_or_default();
                if !key_ok || !prompt.ends_with("Headline: Carbon news") {
                    return (StatusCode::BAD_REQUEST, axum::Json(json!({}))).into_response();
                }
                (
                    StatusCode::OK,
                    axum::Json(json!({"candidates": [{"content": {"parts": [
                        {"text": "{\"category\": "}, {"text": "\"carbon\"}"}
                    ]}}]})),
                )
                    .into_response()
            },
        ),
    );
    let base = serve(router).await;
    let client = GeminiClient::new(Secret::new("fixture-key"), &base).unwrap();
    let text = client
        .generate("prefix\nHeadline: Carbon news", Duration::from_secs(15))
        .await
        .unwrap();
    assert_eq!(text, "{\"category\": \"carbon\"}");
}

#[tokio::test]
async fn the_gemini_client_classifies_refusals_and_retries_transient_failures() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    let hits = Arc::new(AtomicU32::new(0));
    let counter = hits.clone();
    let router = Router::new().route(
        "/v1beta/models/gemini-2.5-flash-lite:generateContent",
        axum::routing::post(move || {
            let counter = counter.clone();
            async move {
                // First call: a server error. Second call: success.
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    (StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({}))).into_response()
                } else {
                    (
                        StatusCode::OK,
                        axum::Json(
                            json!({"candidates": [{"content": {"parts": [{"text": "ok"}]}}]}),
                        ),
                    )
                        .into_response()
                }
            }
        }),
    );
    let base = serve(router).await;
    let client = GeminiClient::new(Secret::new("k"), &base).unwrap();
    assert_eq!(
        client.generate("p", Duration::from_secs(15)).await,
        Ok("ok".to_owned())
    );
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    for (status, body, expected) in [
        (403, json!({}), ModelError::Auth),
        (401, json!({}), ModelError::Auth),
        (
            400,
            json!({"error": {"status": "INVALID_ARGUMENT", "message": "API key was reported as leaked."}}),
            ModelError::Auth,
        ),
        (
            400,
            json!({"error": {"status": "PERMISSION_DENIED", "message": "x"}}),
            ModelError::Auth,
        ),
        (
            400,
            json!({"error": {"message": "bad request"}}),
            ModelError::Other,
        ),
        (404, json!({}), ModelError::Other),
    ] {
        let router =
            Router::new().route(
                "/v1beta/models/gemini-2.5-flash-lite:generateContent",
                axum::routing::post(move || {
                    let body = body.clone();
                    async move {
                        (StatusCode::from_u16(status).unwrap(), axum::Json(body)).into_response()
                    }
                }),
            );
        let base = serve(router).await;
        let client = GeminiClient::new(Secret::new("k"), &base).unwrap();
        assert_eq!(
            client.generate("p", Duration::from_secs(15)).await,
            Err(expected),
            "status {status}"
        );
    }
    // A budget below the retry delay cannot make a call.
    let client = GeminiClient::new(Secret::new("k"), "http://127.0.0.1:9").unwrap();
    assert_eq!(
        client.generate("p", Duration::from_millis(100)).await,
        Err(ModelError::Other)
    );
}

// ---------------------------------------------------------------------------
// Configuration and the command shell
// ---------------------------------------------------------------------------

fn environment(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn configuration_reads_the_three_settings_and_refuses_a_foreign_webz_url() {
    let config = NewsConfig::from_environment(&environment(&[])).unwrap();
    assert_eq!(config, NewsConfig::default());

    let config = NewsConfig::from_environment(&environment(&[
        ("WEBZ_API_TOKEN", "t"),
        ("GEMINI_API_KEY", "g"),
        ("WEBZ_API_TIMEOUT_SECONDS", "12.5"),
    ]))
    .unwrap();
    let webz = config.webz.expect("a token configures Webz");
    assert_eq!(webz.url, WEBZ_DEFAULT_URL);
    assert_eq!(webz.timeout, Duration::from_millis(12_500));
    assert_eq!(config.gemini_api_key.unwrap().expose(), "g");

    // An empty token or key is no credential.
    let config = NewsConfig::from_environment(&environment(&[
        ("WEBZ_API_TOKEN", ""),
        ("GEMINI_API_KEY", ""),
    ]))
    .unwrap();
    assert_eq!(config, NewsConfig::default());

    for url in [
        "https://api.webz.io/filterWebContent",
        "  https://api.webz.io/filterWebContent  ",
        "https://API.WEBZ.IO:443/filterWebContent",
    ] {
        assert!(
            NewsConfig::from_environment(&environment(&[
                ("WEBZ_API_TOKEN", "t"),
                ("WEBZ_API_URL", url)
            ]))
            .is_ok(),
            "{url}"
        );
    }
    for url in [
        "http://api.webz.io/filterWebContent",
        "https://api.webz.io/other",
        "https://api.webz.io/filterWebContent?x=1",
        "https://api.webz.io/filterWebContent#f",
        "https://api.webz.io:8443/filterWebContent",
        "https://user@api.webz.io/filterWebContent",
        "https://evil.example.org/filterWebContent",
        "https://api.webz.io.evil.example.org/filterWebContent",
        "not a url",
        "",
    ] {
        assert_eq!(
            NewsConfig::from_environment(&environment(&[
                ("WEBZ_API_TOKEN", "t"),
                ("WEBZ_API_URL", url)
            ])),
            Err(Exit::Precondition),
            "{url}"
        );
    }
    for timeout in ["0", "-1", "30.5", "nan", "inf", "soon", ""] {
        assert_eq!(
            NewsConfig::from_environment(&environment(&[("WEBZ_API_TIMEOUT_SECONDS", timeout)])),
            Err(Exit::Precondition),
            "{timeout}"
        );
    }
    assert!(
        NewsConfig::from_environment(&environment(&[("WEBZ_API_TIMEOUT_SECONDS", "30")])).is_ok()
    );
}

fn job_args(environment: Environment, release_sha: &str, apply: bool) -> JobArgs {
    JobArgs {
        kind: JobKind::News,
        identity: IdentityArgs {
            environment,
            release_sha: release_sha.into(),
        },
        apply,
    }
}

/// One private scratch directory, removed with its contents.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("verdaxis-news-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&path).expect("create a scratch directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn job_vars(directory: &std::path::Path) -> HashMap<String, String> {
    environment(&[
        ("ENVIRONMENT", "staging"),
        ("RELEASE_SHA", SHA),
        ("DEPLOY_STATE_DIR", directory.to_str().unwrap()),
    ])
}

/// Run the command with an opener that records whether the job got as far as
/// opening a database context. The opener refuses, so no database is needed.
async fn run_recording(
    args: &JobArgs,
    vars: &HashMap<String, String>,
    state: &std::path::Path,
) -> (Exit, bool) {
    let opened = std::sync::atomic::AtomicBool::new(false);
    let exit = run_with(args, vars, state, |_, _| {
        opened.store(true, std::sync::atomic::Ordering::SeqCst);
        async { Err(Exit::Precondition) }
    })
    .await;
    (exit, opened.load(std::sync::atomic::Ordering::SeqCst))
}

#[tokio::test]
async fn the_job_refuses_wrong_identity_and_deploy_state_before_any_work() {
    let scratch = Scratch::new();
    let state = scratch.0.join("staging.state");
    let args = job_args(Environment::Staging, SHA, true);

    // Absent state, matching identity, `--apply`: the job reaches its opener.
    assert_eq!(
        run_recording(&args, &job_vars(&scratch.0), &state).await,
        (Exit::Precondition, true),
        "the opener is the refusal here"
    );

    // A present state file refuses, and so does a dangling link.
    std::fs::write(&state, b"blocked").unwrap();
    assert_eq!(
        run_recording(&args, &job_vars(&scratch.0), &state).await,
        (Exit::Precondition, false)
    );
    std::fs::remove_file(&state).unwrap();
    std::os::unix::fs::symlink(scratch.0.join("missing-target"), &state).unwrap();
    assert!(!state.exists(), "the link dangles");
    assert_eq!(
        run_recording(&args, &job_vars(&scratch.0), &state).await,
        (Exit::Precondition, false)
    );
    std::fs::remove_file(&state).unwrap();

    // The deployed environment and release must equal the command's.
    let mut other_environment = job_vars(&scratch.0);
    other_environment.insert("ENVIRONMENT".into(), "production".into());
    let mut other_release = job_vars(&scratch.0);
    other_release.insert("RELEASE_SHA".into(), "f".repeat(40));
    let mut no_environment = job_vars(&scratch.0);
    no_environment.remove("ENVIRONMENT");
    let mut no_release = job_vars(&scratch.0);
    no_release.remove("RELEASE_SHA");
    for vars in [other_environment, other_release, no_environment, no_release] {
        assert_eq!(
            run_recording(&args, &vars, &state).await,
            (Exit::Precondition, false)
        );
    }
    // A release that is not a full lowercase SHA never matches.
    for release in ["0123456789abcdef", &SHA.to_uppercase(), ""] {
        assert_eq!(
            run_recording(
                &job_args(Environment::Staging, release, true),
                &job_vars(&scratch.0),
                &state
            )
            .await,
            (Exit::Precondition, false),
            "{release:?}"
        );
    }
    // A relative state path is refused: it would name a different place from
    // every working directory.
    assert_eq!(
        run_recording(
            &args,
            &job_vars(&scratch.0),
            std::path::Path::new("staging.state")
        )
        .await,
        (Exit::Precondition, false)
    );

    // Identity is checked before `--apply`: a wrong identity is exit 3 even
    // without the flag; with the right identity the missing flag is exit 2.
    let without_apply = job_args(Environment::Staging, SHA, false);
    let mut wrong = job_vars(&scratch.0);
    wrong.insert("ENVIRONMENT".into(), "production".into());
    assert_eq!(
        run_recording(&without_apply, &wrong, &state).await,
        (Exit::Precondition, false)
    );
    assert_eq!(
        run_recording(&without_apply, &job_vars(&scratch.0), &state).await,
        (Exit::Usage, false)
    );
}

#[test]
fn the_deploy_state_directory_must_be_set_and_absolute() {
    let state =
        |pairs: &[(&str, &str)]| deploy_state_path(&environment(pairs), Environment::Staging);
    assert_eq!(
        state(&[("DEPLOY_STATE_DIR", "/run/deploy")]),
        Ok(std::path::PathBuf::from("/run/deploy/staging.state"))
    );
    assert_eq!(state(&[]), Err(Exit::Precondition));
    assert_eq!(
        state(&[("DEPLOY_STATE_DIR", "relative/dir")]),
        Err(Exit::Precondition)
    );
    assert_eq!(
        deploy_state_path(
            &environment(&[("DEPLOY_STATE_DIR", "/run/deploy")]),
            Environment::Production
        ),
        Ok(std::path::PathBuf::from("/run/deploy/production.state"))
    );
}

#[test]
fn a_variable_that_is_not_unicode_is_left_out_without_a_panic() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let variables = vec![
        (OsString::from("ENVIRONMENT"), OsString::from("staging")),
        (
            OsString::from("BAD_VALUE"),
            OsString::from_vec(vec![0x66, 0xff, 0x6f]),
        ),
        (
            OsString::from_vec(vec![0x4b, 0xfe]),
            OsString::from("value"),
        ),
        (OsString::from("WEBZ_API_TOKEN"), OsString::from("token")),
    ];
    let vars = environment_from_os(variables.into_iter());
    assert_eq!(vars.len(), 2);
    assert_eq!(vars["ENVIRONMENT"], "staging");
    assert_eq!(vars["WEBZ_API_TOKEN"], "token");
}

// ---------------------------------------------------------------------------
// Publication dates outside the supported range
// ---------------------------------------------------------------------------

#[test]
fn a_publication_time_outside_years_1_to_9999_becomes_the_run_time() {
    let year = |year: i32| Utc.with_ymd_and_hms(year, 6, 15, 0, 0, 0).single().unwrap();
    for inside in [1, 1970, 2026, 9999] {
        assert_eq!(bounded_published(Some(year(inside)), now()), year(inside));
    }
    for outside in [-5000, -1, 0, 10_000, 100_000] {
        assert_eq!(
            bounded_published(Some(year(outside)), now()),
            now(),
            "{outside}"
        );
    }
    assert_eq!(bounded_published(None, now()), now());
    // A time in year 1 whose offset conversion would leave the range.
    let edge = DateTime::parse_from_rfc3339("0001-01-01T00:30:00+05:00")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(bounded_published(Some(edge), now()), now());
}

#[test]
fn a_far_past_or_far_future_webz_date_is_the_run_time() {
    for text in [
        "-5000-01-01T00:00:00+0000",
        "-0001-06-15T12:00:00.000+0000",
        "+10000-01-01T00:00:00+0000",
        "0000-01-01T00:00:00Z",
        "0001-01-01T00:30:00+05:00",
        "99999-01-01T00:00:00+0000",
    ] {
        assert_eq!(
            parse_webz_published(Some(&json!(text)), now()),
            now(),
            "{text}"
        );
    }
    // The same rule through a whole post: the row keeps a storable date.
    let item = validated_webz_post(
        &json!({
            "title": "Dated long ago",
            "url": "https://news.example.org/old",
            "published": "-5000-01-01T00:00:00+0000"
        }),
        now(),
    )
    .unwrap();
    assert_eq!(item.published_at, now());
}

#[test]
fn an_rss_date_outside_the_supported_range_is_the_run_time() {
    let spec = feed("Pub", "https://publisher.test/rss", "publisher.test");
    for date in [
        "01 Jan -5000 00:00:00 +0000",
        "Fri, 30 Oct 10000 08:00:00 GMT",
        "Fri, 30 Oct 0000 08:00:00 GMT",
    ] {
        let body = format!(
            "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>f</title>\
             <link>https://publisher.test</link><description>d</description>\
             <item><title>Old</title><link>https://publisher.test/o</link>\
             <pubDate>{date}</pubDate></item></channel></rss>"
        );
        let items = parse_feed(body.as_bytes(), &spec, now()).unwrap();
        assert_eq!(items.len(), 1, "{date}");
        let year = chrono::Datelike::year(&items[0].published_at);
        assert!((1..=9999).contains(&year), "{date}: year {year}");
    }
}

#[test]
fn the_advisory_key_is_the_one_the_python_job_used() {
    assert_eq!(NEWS_REFRESH_ADVISORY_LOCK_ID, 6_216_461_178_696_259_923);
}

#[test]
fn the_default_feed_list_is_the_five_maritime_publishers() {
    let feeds = default_feeds();
    let names: Vec<&str> = feeds.iter().map(|feed| feed.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "TradeWinds",
            "Ship & Bunker",
            "Splash247",
            "Maritime Executive",
            "Hellenic Shipping News"
        ]
    );
    for feed in &feeds {
        assert!(feed.url.starts_with("https://"));
        assert!(
            validated_article_url(
                &format!("https://{}/x", feed.article_domains[0]),
                &feed.article_domains
            )
            .is_some()
        );
    }
}
