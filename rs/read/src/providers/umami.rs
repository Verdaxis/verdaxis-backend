//! Stateless Umami aggregate adapter.
//!
//! One call authenticates, reads the typed aggregate it needs and discards the
//! credential and every intermediate document. There is no aggregate cache, no
//! token cache, no single-flight map, no per-key waiter state and no sender: a
//! second call authenticates and fetches again. Nothing is written to a
//! database, and no raw upstream document leaves this module.
//!
//! Bounds, all provider-enforced rather than advisory:
//! - one response body of at most 1,000,000 bytes, refused while streaming;
//! - a request timeout above zero and at most ten seconds;
//! - redirects are never followed, so a bearer token cannot travel to a new
//!   host;
//! - at most 366 daily points, 20 ranking rows and 500 property rows;
//! - at most twelve distinct requested events, each fetched through the
//!   verified `event-data/events?event=<name>` form.
//!
//! At most one forced re-authentication happens, after which a repeated 401 or
//! any 403 is a typed authentication failure. Every failure maps onto the
//! closed diagnostic domain; no upstream text, credential or body is kept.

use std::{
    fmt,
    sync::Arc,
    time::Duration,
};

use chrono::{NaiveDate, Utc};
use reqwest::StatusCode as HttpStatus;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::analytics::{
    behavioral::{
        AllowedPropertyAggregate, BehavioralWindow, CoreMetrics, MAX_PROPERTY_ROWS,
        PropertyAvailability, WindowRows, assemble,
    },
    events::{AllowedProperty, AllowedPropertySet, DomainCatalog, EntryPage, ReferrerCategory, RegisteredEvent},
    query::AnalyticsQuery,
    types::{
        AnalyticsDiagnostic, ComponentStatus, ProviderComponent,
    },
};

/// Hard provider body cap, in bytes. It is the same limit in every call.
pub const MAX_RESPONSE_BYTES: usize = 1_000_000;
/// The longest accepted configured timeout, in seconds.
pub const MAX_TIMEOUT_SECONDS: u64 = 10;
/// The longest accepted token, in bytes.
const MAX_TOKEN_BYTES: usize = 16_384;

/// The provider budget. Only the timeout is configurable; the body cap is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UmamiPolicy {
    pub timeout: Duration,
}

impl Default for UmamiPolicy {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(2),
        }
    }
}

/// Configuration for one adapter. Credentials are server-only and never
/// appear in `Debug`, a response or a log.
#[derive(Clone)]
pub struct UmamiConfig {
    pub enabled: bool,
    pub base_url: Option<String>,
    pub website_id: Option<String>,
    pub api_username: Option<String>,
    pub api_password: Option<Zeroizing<String>>,
    pub policy: UmamiPolicy,
    pub catalog: Arc<DomainCatalog>,
}

impl fmt::Debug for UmamiConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UmamiConfig")
            .field("enabled", &self.enabled)
            .field("base_url", &self.base_url)
            .field("website_id", &self.website_id)
            .field("api_username", &self.api_username)
            .field("api_password", &self.api_password.as_ref().map(|_| "[redacted]"))
            .field("policy", &self.policy)
            .finish()
    }
}

struct Inner {
    http: reqwest::Client,
    base: Option<String>,
    website: Option<String>,
    username: Option<String>,
    password: Option<Zeroizing<String>>,
    enabled: bool,
    catalog: Arc<DomainCatalog>,
}

/// A shared HTTP pool and the static configuration. It holds no token, no
/// result and no per-key state, so it is safe to share across requests.
#[derive(Clone)]
pub struct UmamiClient {
    inner: Arc<Inner>,
}

impl fmt::Debug for UmamiClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UmamiClient")
            .field("base_url", &self.inner.base)
            .field("website_id", &self.inner.website)
            .field("enabled", &self.inner.enabled)
            .finish()
    }
}

impl UmamiClient {
    /// Build a client. The timeout, base URL and credentials are validated
    /// once here; an unusable value is an error, not a runtime surprise.
    pub fn new(config: UmamiConfig) -> Result<Self, &'static str> {
        let timeout = config.policy.timeout;
        if timeout.is_zero() || timeout > Duration::from_secs(MAX_TIMEOUT_SECONDS) {
            return Err("Umami timeout must be greater than zero and at most 10 seconds");
        }
        let base = normalize_base(config.base_url.as_deref())?;
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Umami HTTP client could not be built")?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                base,
                website: nonempty(config.website_id),
                username: nonempty(config.api_username),
                password: config
                    .api_password
                    .filter(|password| !password.trim().is_empty()),
                enabled: config.enabled,
                catalog: config.catalog,
            }),
        })
    }

    pub fn catalog(&self) -> &DomainCatalog {
        &self.inner.catalog
    }

    /// Fetch one half-open UTC window.
    ///
    /// Every requested (event, property) pair receives exactly one result.
    /// Core failure makes the whole window unavailable. A failed property
    /// breakdown degrades the window to partial and keeps the core.
    pub async fn window(
        &self,
        query: &AnalyticsQuery,
        properties: AllowedPropertySet,
    ) -> BehavioralWindow {
        let observed_at = Utc::now();
        let placeholder = unavailable_properties(&properties);
        let diagnostic = match self.configuration() {
            Ok(()) => None,
            Err(diagnostic) => Some(diagnostic),
        };
        if let Some(diagnostic) = diagnostic {
            return BehavioralWindow::unavailable(
                query.start,
                query.end,
                observed_at,
                diagnostic,
                placeholder,
            );
        }
        match self.fetch(query, &properties).await {
            Ok((rows, property_results, failures)) => assemble(
                query.start,
                query.end,
                observed_at,
                rows,
                property_results,
                failures,
            ),
            Err(diagnostic) => BehavioralWindow::unavailable(
                query.start,
                query.end,
                observed_at,
                diagnostic,
                placeholder,
            ),
        }
    }

    fn configuration(&self) -> Result<(), AnalyticsDiagnostic> {
        if !self.inner.enabled {
            return Err(AnalyticsDiagnostic::Disabled);
        }
        if self.inner.base.is_none()
            || self.inner.website.is_none()
            || self.inner.username.is_none()
            || self.inner.password.is_none()
        {
            return Err(AnalyticsDiagnostic::Configuration);
        }
        Ok(())
    }

    async fn fetch(
        &self,
        query: &AnalyticsQuery,
        properties: &AllowedPropertySet,
    ) -> Result<(WindowRows, Vec<PropertyAvailability>, usize), AnalyticsDiagnostic> {
        let base = self.base()?;
        let website = self
            .inner
            .website
            .as_deref()
            .ok_or(AnalyticsDiagnostic::Configuration)?;
        let site = format!("{base}/api/websites/{website}");
        let params = vec![
            ("startAt".to_owned(), query.start.timestamp_millis().to_string()),
            ("endAt".to_owned(), query.end.timestamp_millis().to_string()),
        ];

        // One request at a time: a single token is threaded through, and at
        // most one forced re-authentication can happen per fetch.
        let mut token = self.authenticate().await?;
        let stats = self
            .authorized_get(&format!("{site}/stats"), params.clone(), &mut token)
            .await?;
        let mut pageview_params = params.clone();
        pageview_params.push(("unit".to_owned(), "day".to_owned()));
        pageview_params.push(("timezone".to_owned(), "UTC".to_owned()));
        let pageviews = self
            .authorized_get(&format!("{site}/pageviews"), pageview_params.clone(), &mut token)
            .await?;
        let series = self
            .authorized_get(&format!("{site}/events/series"), pageview_params, &mut token)
            .await?;
        let mut entry_params = params.clone();
        entry_params.push(("type".to_owned(), "entry".to_owned()));
        entry_params.push(("limit".to_owned(), "10".to_owned()));
        let entries = self
            .authorized_get(&format!("{site}/metrics"), entry_params, &mut token)
            .await?;
        let mut referrer_params = params.clone();
        referrer_params.push(("type".to_owned(), "referrer".to_owned()));
        referrer_params.push(("limit".to_owned(), "10".to_owned()));
        let referrers = self
            .authorized_get(&format!("{site}/metrics"), referrer_params, &mut token)
            .await?;

        let core = decode_core(&stats)?;
        let sessions = decode_sessions(&pageviews)?;
        let events = decode_event_series(&series)?;
        let entries = decode_metrics(&entries, |raw| EntryPage::from_raw_path(raw))?;
        let referrers = decode_metrics(&referrers, ReferrerCategory::from_raw)?;

        let mut property_results = Vec::with_capacity(properties.pairs().len());
        let mut failures = 0;
        for event in properties.events() {
            let requested = properties.properties_for(*event);
            let mut event_params = params.clone();
            event_params.push(("event".to_owned(), event.name().to_owned()));
            let outcome = self
                .authorized_get(&format!("{site}/event-data/events"), event_params, &mut token)
                .await;
            match outcome {
                Ok(payload) => {
                    let rows = decode_property_rows(*event, &payload, &self.inner.catalog)?;
                    for property in requested {
                        let selected: Vec<_> = rows
                            .iter()
                            .filter(|row| row.property == property)
                            .cloned()
                            .collect();
                        property_results.push(PropertyAvailability {
                            event: *event,
                            property,
                            result: ProviderComponent {
                                status: ComponentStatus::Available,
                                diagnostic: None,
                                data: Some(selected),
                            },
                        });
                    }
                }
                Err(diagnostic) => {
                    failures += 1;
                    for property in requested {
                        property_results.push(PropertyAvailability {
                            event: *event,
                            property,
                            result: ProviderComponent {
                                status: ComponentStatus::Unavailable,
                                diagnostic: Some(diagnostic),
                                data: None,
                            },
                        });
                    }
                }
            }
        }

        Ok((
            WindowRows {
                stats: core,
                sessions,
                events,
                entries,
                referrers,
            },
            property_results,
            failures,
        ))
    }

    fn base(&self) -> Result<String, AnalyticsDiagnostic> {
        self.inner
            .base
            .clone()
            .ok_or(AnalyticsDiagnostic::Configuration)
    }

    /// One authentication for this request. Nothing is retained afterwards.
    async fn authenticate(&self) -> Result<Zeroizing<String>, AnalyticsDiagnostic> {
        let (base, username, password) = match (
            &self.inner.base,
            &self.inner.username,
            &self.inner.password,
        ) {
            (Some(base), Some(username), Some(password)) => (base, username, password),
            _ => return Err(AnalyticsDiagnostic::Configuration),
        };
        let response = self
            .inner
            .http
            .post(format!("{base}/api/auth/login"))
            .json(&serde_json::json!({
                "username": username,
                "password": password.as_str(),
            }))
            .send()
            .await
            .map_err(classify)?;
        let status = response.status();
        if status == HttpStatus::UNAUTHORIZED || status == HttpStatus::FORBIDDEN {
            return Err(AnalyticsDiagnostic::Authentication);
        }
        if !status.is_success() {
            return Err(AnalyticsDiagnostic::Upstream);
        }
        let document = self.bounded_json(response).await?;
        let token = document
            .get("token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty() && token.len() <= MAX_TOKEN_BYTES)
            .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
        Ok(Zeroizing::new(token.to_owned()))
    }

    /// One authenticated GET with at most one forced re-authentication.
    async fn authorized_get(
        &self,
        url: &str,
        params: Vec<(String, String)>,
        token: &mut Zeroizing<String>,
    ) -> Result<Value, AnalyticsDiagnostic> {
        for attempt in 0..2 {
            let response = self
                .inner
                .http
                .get(with_query(url, &params)?)
                .bearer_auth(token.as_str())
                .send()
                .await
                .map_err(classify)?;
            let status = response.status();
            if status == HttpStatus::UNAUTHORIZED && attempt == 0 {
                *token = self.authenticate().await?;
                continue;
            }
            if status == HttpStatus::UNAUTHORIZED || status == HttpStatus::FORBIDDEN {
                return Err(AnalyticsDiagnostic::Authentication);
            }
            if !status.is_success() {
                return Err(AnalyticsDiagnostic::Upstream);
            }
            return self.bounded_json(response).await;
        }
        Err(AnalyticsDiagnostic::Authentication)
    }

    /// Read a JSON body with a hard byte cap. The announced length is checked
    /// first, then every streamed chunk, so a chunked body cannot exceed it.
    async fn bounded_json(
        &self,
        mut response: reqwest::Response,
    ) -> Result<Value, AnalyticsDiagnostic> {
        if let Some(length) = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
        {
            let length: usize = length
                .parse()
                .map_err(|_| AnalyticsDiagnostic::MalformedResponse)?;
            if length > MAX_RESPONSE_BYTES {
                return Err(AnalyticsDiagnostic::MalformedResponse);
            }
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(classify)? {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(AnalyticsDiagnostic::MalformedResponse);
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| AnalyticsDiagnostic::MalformedResponse)
    }
}

/// Append query parameters without relying on an optional client feature.
fn with_query(url: &str, params: &[(String, String)]) -> Result<url::Url, AnalyticsDiagnostic> {
    let mut url = url::Url::parse(url).map_err(|_| AnalyticsDiagnostic::Configuration)?;
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in params {
            pairs.append_pair(key, value);
        }
    }
    Ok(url)
}

fn classify(error: reqwest::Error) -> AnalyticsDiagnostic {
    if error.is_timeout() {
        AnalyticsDiagnostic::Timeout
    } else {
        AnalyticsDiagnostic::Upstream
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

/// Validate and normalize the collector base URL. A blank value means "not
/// configured"; anything else must be an absolute HTTP(S) URL without
/// embedded credentials.
fn normalize_base(value: Option<&str>) -> Result<Option<String>, &'static str> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let url = url::Url::parse(raw).map_err(|_| "Umami base URL must be absolute")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Umami base URL must be an absolute HTTP(S) URL");
    }
    if url.host_str().is_none() {
        return Err("Umami base URL must be an absolute HTTP(S) URL");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Umami base URL must not contain credentials");
    }
    Ok(Some(raw.trim_end_matches('/').to_owned()))
}

fn unavailable_properties(properties: &AllowedPropertySet) -> Vec<PropertyAvailability> {
    properties
        .pairs()
        .iter()
        .map(|(event, property)| PropertyAvailability {
            event: *event,
            property: *property,
            result: ProviderComponent {
                status: ComponentStatus::Unavailable,
                diagnostic: None,
                data: None,
            },
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Decoding. Every shape drift is `MalformedResponse`; nothing is invented.
// ---------------------------------------------------------------------------

/// A non-negative count. Booleans, strings, fractional values above zero and
/// negative values are not counts: a negative or fractional count is invalid
/// data, never a genuine zero.
fn count(value: Option<&Value>) -> Result<u64, AnalyticsDiagnostic> {
    let value = value.ok_or(AnalyticsDiagnostic::MalformedResponse)?;
    let number = value
        .as_f64()
        .filter(|number| number.is_finite() && *number >= 0.0)
        .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
    Ok(number.trunc() as u64)
}

fn decode_core(document: &Value) -> Result<CoreMetrics, AnalyticsDiagnostic> {
    let object = document
        .as_object()
        .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
    let visitors = count(object.get("visitors"))?;
    let visits = count(object.get("visits"))?;
    let pageviews = count(object.get("pageviews"))?;
    let total_time_seconds = count(object.get("totaltime"))?;
    let average_session_duration_seconds = (visits > 0).then(|| {
        (Decimal::from(total_time_seconds) / Decimal::from(visits))
            .round_dp_with_strategy(1, RoundingStrategy::MidpointNearestEven)
    });
    Ok(CoreMetrics {
        visitors: Some(visitors),
        visits: Some(visits),
        pageviews: Some(pageviews),
        total_time_seconds: Some(total_time_seconds),
        average_session_duration_seconds,
    })
}

fn decode_date(value: Option<&Value>) -> Result<NaiveDate, AnalyticsDiagnostic> {
    let raw = value
        .and_then(Value::as_str)
        .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
    let day = raw.get(..10).unwrap_or(raw);
    NaiveDate::parse_from_str(day, "%Y-%m-%d").map_err(|_| AnalyticsDiagnostic::MalformedResponse)
}

fn decode_sessions(document: &Value) -> Result<Vec<(NaiveDate, u64)>, AnalyticsDiagnostic> {
    let rows = document
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
    let mut sessions = Vec::with_capacity(rows.len().min(366));
    for row in rows {
        let object = row
            .as_object()
            .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
        sessions.push((
            decode_date(object.get("x"))?,
            count(object.get("y"))?,
        ));
    }
    Ok(sessions)
}

fn decode_event_series(rows: &Value) -> Result<Vec<(NaiveDate, RegisteredEvent, u64)>, AnalyticsDiagnostic> {
    let rows = rows
        .as_array()
        .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
    let mut events = Vec::with_capacity(rows.len().min(366));
    for row in rows {
        let object = row
            .as_object()
            .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
        let name = object
            .get("x")
            .and_then(Value::as_str)
            .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
        let date = decode_date(object.get("t"))?;
        let value = count(object.get("y"))?;
        // Unknown event names are discarded before aggregation, never echoed.
        if let Some(event) = RegisteredEvent::from_name(name) {
            events.push((date, event, value));
        }
    }
    Ok(events)
}

fn decode_metrics<T: Copy>(
    rows: &Value,
    classify: impl Fn(&str) -> T,
) -> Result<Vec<(T, u64)>, AnalyticsDiagnostic> {
    let rows = rows
        .as_array()
        .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
    let mut metrics = Vec::with_capacity(rows.len().min(10));
    for row in rows.iter().take(10) {
        let object = row
            .as_object()
            .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
        let label = object
            .get("x")
            .and_then(Value::as_str)
            .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
        metrics.push((classify(label), count(object.get("y"))?));
    }
    Ok(metrics)
}

/// Decode one event's property rows into allowlisted, aggregated values.
fn decode_property_rows(
    event: RegisteredEvent,
    payload: &Value,
    catalog: &DomainCatalog,
) -> Result<Vec<AllowedPropertyAggregate>, AnalyticsDiagnostic> {
    let rows = payload
        .as_array()
        .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
    let mut aggregates: Vec<(AllowedProperty, String, u64)> = Vec::new();
    for row in rows.iter().take(MAX_PROPERTY_ROWS) {
        let object = row
            .as_object()
            .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
        let property_name = object
            .get("propertyName")
            .and_then(Value::as_str)
            .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
        let value = object
            .get("propertyValue")
            .and_then(Value::as_str)
            .ok_or(AnalyticsDiagnostic::MalformedResponse)?;
        let total = count(object.get("total"))?;
        let Some(property) = AllowedProperty::from_name(property_name) else {
            continue;
        };
        if !event.properties().contains(&property) {
            continue;
        }
        let Some(value) = crate::analytics::events::AllowedPropertyValue::parse(
            event, property, value, catalog,
        ) else {
            continue;
        };
        match aggregates
            .iter_mut()
            .find(|(candidate, raw, _)| *candidate == property && raw == value.as_str())
        {
            Some((_, _, running)) => *running += total,
            None => aggregates.push((property, value.as_str().to_owned(), total)),
        }
    }
    aggregates.sort_by(|left, right| {
        right
            .2
            .cmp(&left.2)
            .then_with(|| left.0.name().cmp(right.0.name()))
            .then_with(|| left.1.cmp(&right.1))
    });
    aggregates.truncate(MAX_PROPERTY_ROWS);
    Ok(aggregates
        .into_iter()
        .filter_map(|(property, raw, total)| {
            let value = crate::analytics::events::AllowedPropertyValue::parse(
                event, property, &raw, catalog,
            )?;
            Some(AllowedPropertyAggregate {
                event,
                property,
                count: crate::analytics::events::named_cell(value.as_str(), total),
                value,
            })
        })
        .collect())
}
