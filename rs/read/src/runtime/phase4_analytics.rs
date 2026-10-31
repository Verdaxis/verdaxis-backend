//! Shared bounded provider and authoritative fact-tab composition.
use crate::{
    ApiError, AppState,
    analytics::{
        self,
        marketplace::{
            BehavioralRead, ProviderFuture, ReferenceFuture, ReferenceQuote, ReferenceRead,
            ReferenceRequest,
        },
        policy::AllowedPropertySet,
        query::{AnalyticsQuery, QueryInput},
    },
    providers::umami::{UmamiClient, UmamiConfig, UmamiPolicy},
};
use axum::{
    Json, Router,
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::Utc;
use std::sync::Arc;

#[derive(Clone)]
pub struct Provider(pub UmamiClient);
impl BehavioralRead for Provider {
    fn window<'a>(
        &'a self,
        query: &'a AnalyticsQuery,
        properties: &'a AllowedPropertySet,
    ) -> ProviderFuture<'a, analytics::types::BehavioralWindow> {
        Box::pin(async move { self.0.window(query, properties.clone()).await })
    }
}
impl analytics::usage::BehavioralUsageSource for Provider {
    fn window(
        &self,
        window: analytics::usage::UsageWindow,
    ) -> analytics::usage::BehavioralFuture<'_> {
        Box::pin(async move {
            let query = analytics::query::validate_query(QueryInput {
                start: Some(window.start.to_rfc3339()),
                end: Some(window.end.to_rfc3339()),
                compare: Some("false".into()),
                ..Default::default()
            })
            .expect("validated usage window");
            self.0
                .window(
                    &query,
                    AllowedPropertySet::new([]).expect("empty properties"),
                )
                .await
        })
    }
}
pub fn provider(app: &AppState) -> Provider {
    let value = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    let config = UmamiConfig {
        enabled: value("UMAMI_ENABLED").is_some_and(|v| v == "true"),
        base_url: value("UMAMI_BASE_URL"),
        website_id: value("UMAMI_WEBSITE_ID"),
        api_username: value("UMAMI_API_USERNAME"),
        api_password: value("UMAMI_API_PASSWORD").map(Into::into),
        policy: UmamiPolicy::default(),
        catalog: Arc::new(analytics::events::DomainCatalog::from_policy(
            &app.policy,
            crate::market::terms::ISO_3166_1_ALPHA_2,
        )),
    };
    Provider(UmamiClient::new(config).expect("prepared aggregate provider configuration"))
}
struct Reference;
impl ReferenceRead for Reference {
    fn quotes<'a>(
        &'a self,
        conn: &'a mut sqlx::PgConnection,
        requests: &'a [ReferenceRequest],
    ) -> ReferenceFuture<'a> {
        Box::pin(async move {
            let keys=serde_json::json!(requests.iter().map(|r|serde_json::json!({"market_product":r.product_code,"delivery_point_id":r.delivery_point_id,"availability_window":r.window})).collect::<Vec<_>>());
            let quotes = crate::routes::curves_canonical::benchmarks::load(conn, &keys).await?;
            let mut result = Vec::new();
            for request in requests {
                if let Some(stored) = quotes.get(&(
                    request.product_code.clone(),
                    request.delivery_point_id.to_string(),
                    request.window.clone(),
                )) {
                    let price = stored["price"]
                        .as_str()
                        .and_then(|p| p.parse().ok())
                        .ok_or_else(ApiError::internal)?;
                    result.push(ReferenceQuote {
                        product_code: request.product_code.clone(),
                        delivery_point_id: request.delivery_point_id,
                        window: request.window.clone(),
                        benchmark_price_usd_per_mt: price,
                        source: stored["source"].as_str().unwrap_or("benchmark").into(),
                        generated_at: Utc::now(),
                        observed_at: stored["observed_at"].as_str().and_then(|v| v.parse().ok()),
                    });
                }
            }
            Ok(result)
        })
    }
}
#[derive(Clone)]
struct Authority {
    app: AppState,
    verification: Arc<crate::auth::VerificationSettings>,
    limits: crate::auth::limits::SharedLimits,
    provider: Provider,
}
pub fn routes(
    app: &AppState,
    limits: crate::auth::limits::SharedLimits,
    provider: Provider,
) -> Router {
    routes_with_provider(app, limits, provider)
}
pub fn routes_with_provider(
    app: &AppState,
    limits: crate::auth::limits::SharedLimits,
    provider: Provider,
) -> Router {
    let settings = app.phase2.as_ref().expect("business credentials");
    Router::new()
        .merge(analytics::marketplace::router(
            app.clone(),
            settings.verification.clone(),
            limits.clone(),
            analytics::marketplace::AnalyticsService {
                behavioral: Arc::new(provider.clone()),
                reference: Arc::new(Reference),
            },
        ))
        .merge(
            Router::new()
                .route(
                    "/api/admin/analytics/product-analytics/overview",
                    get(fact_tab),
                )
                .route(
                    "/api/admin/analytics/product-analytics/activation",
                    get(fact_tab),
                )
                .route(
                    "/api/admin/analytics/product-analytics/engagement",
                    get(fact_tab),
                )
                .route(
                    "/api/admin/analytics/product-analytics/retention",
                    get(fact_tab),
                )
                .with_state(Authority {
                    app: app.clone(),
                    verification: Arc::new(settings.verification.clone()),
                    limits,
                    provider,
                }),
        )
}
async fn fact_tab(State(a): State<Authority>, request: Request) -> Result<Response, ApiError> {
    let now = Utc::now();
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());
    let access = crate::auth::verify_authorization(authorization, &a.verification, now)
        .map_err(crate::business::error::authentication)?;
    let path = request.uri().path().to_owned();
    let template = match path.rsplit('/').next().unwrap_or("") {
        "overview" => "/api/admin/analytics/product-analytics/overview",
        "activation" => "/api/admin/analytics/product-analytics/activation",
        "engagement" => "/api/admin/analytics/product-analytics/engagement",
        _ => "/api/admin/analytics/product-analytics/retention",
    };
    let properties = requested_properties(template.rsplit('/').next().unwrap_or(""));
    let token = format!("usr:{}", access.subject);
    let limits = a.limits.clone();
    crate::auth::run_authenticated(
        &a.app,
        access,
        crate::auth::AuthenticatedRequest::new("GET", template, false),
        move |tx| {
            Box::pin(async move {
                analytics::usage::require_admin(tx.actor())?;
                limits.check_business(
                    "GET",
                    template,
                    crate::auth::limits::BusinessLimitKeys {
                        client: "unknown",
                        user: None,
                        organization: None,
                        token: Some(&token),
                    },
                    now.timestamp().max(0) as u64,
                )
            })
        },
    )
    .await
    .map_err(map_auth)?;
    // Parsed only after the administrator check: authentication (401), then
    // role (403), then input validation (422).
    let query = fact_query(
        request.uri().query().unwrap_or(""),
        template.rsplit('/').next().unwrap_or(""),
    )?;
    let permit = crate::runtime::admission::try_read(&a.app)?;
    let window = a.provider.0.window(&query, properties.clone()).await;
    let previous = if let Some((start, end)) = query.previous {
        let mut q = query.clone();
        q.start = start;
        q.end = end;
        q.compare = false;
        q.previous = None;
        Some(a.provider.0.window(&q, properties.clone()).await)
    } else {
        None
    };
    drop(permit);
    let access = crate::auth::verify_authorization(authorization, &a.verification, Utc::now())
        .map_err(crate::business::error::authentication)?;
    let result = crate::auth::run_authenticated(
        &a.app,
        access,
        crate::auth::AuthenticatedRequest::new("GET", template, false),
        move |tx| {
            Box::pin(async move {
                analytics::usage::require_admin(tx.actor())?;
                let conn = tx.connection();
                let value = match template.rsplit('/').next().unwrap_or("") {
                    "overview" => serde_json::to_value(analytics::overview::overview(
                        &analytics::aggregates::load_overview_facts(conn, &query)
                            .await
                            .map_err(|_| ApiError::database())?,
                        Some(&window),
                        previous.as_ref(),
                        &query,
                        now,
                    )),
                    "activation" => serde_json::to_value(analytics::activation::activation(
                        &analytics::aggregates::load_activation_facts(conn, &query)
                            .await
                            .map_err(|_| ApiError::database())?,
                        Some(&window),
                        &query,
                        now,
                    )),
                    "engagement" => serde_json::to_value(analytics::engagement::engagement(
                        &analytics::aggregates::load_engagement_facts(conn, &query)
                            .await
                            .map_err(|_| ApiError::database())?,
                        Some(&window),
                        &query,
                        now,
                    )),
                    _ => serde_json::to_value(analytics::cohorts::retention(
                        &analytics::aggregates::load_retention_facts(conn, &query)
                            .await
                            .map_err(|_| ApiError::database())?,
                        Some(&window),
                        &query,
                        now,
                    )),
                };
                value.map_err(|_| ApiError::internal())
            })
        },
    )
    .await
    .map_err(map_auth)?;
    Ok(Json(result).into_response())
}
fn map_auth(error: crate::auth::AuthenticatedError) -> ApiError {
    match error {
        crate::auth::AuthenticatedError::Authentication(e) => {
            crate::business::error::authentication(e)
        }
        crate::auth::AuthenticatedError::Database(e) => e,
        _ => ApiError::database(),
    }
}

pub fn fact_query(raw: &str, _tab: &str) -> Result<AnalyticsQuery, ApiError> {
    let pairs: Vec<_> = url::form_urlencoded::parse(raw.as_bytes())
        .into_owned()
        .collect();
    let query = analytics::query::validate_query(QueryInput::from_pairs(
        pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())),
    ))?;
    analytics::marketplace::refuse_reference(&query)?;
    Ok(query)
}
pub fn requested_properties(tab: &str) -> AllowedPropertySet {
    use analytics::policy::{
        AllowedFrontendEvent as Event, AllowedProperty as Property, RegisteredEvent,
    };
    if tab == "engagement" {
        AllowedPropertySet::new([
            (
                RegisteredEvent::Frontend(Event::PlatformNavigation),
                Property::Destination,
            ),
            (
                RegisteredEvent::Frontend(Event::TutorialStepCompleted),
                Property::Step,
            ),
            (
                RegisteredEvent::Frontend(Event::TutorialStepSkipped),
                Property::Step,
            ),
        ])
        .expect("registered engagement properties")
    } else {
        AllowedPropertySet::new([]).expect("empty properties")
    }
}
