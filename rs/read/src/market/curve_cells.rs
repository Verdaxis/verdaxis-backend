//! Canonical mark selection is distinct from the older board's benchmark view.
use super::{Book, DISCLAIMER, decimal, label_policy, midpoint, money, timestamp};
use crate::Json;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde_json::json;

pub fn values(
    book: &Book,
    trade: Option<&Json>,
    benchmark: Option<&Json>,
    indication: &Json,
    band: &Json,
    profile: &str,
    now: DateTime<Utc>,
) -> (Json, Json) {
    let (bid, ask, volume, count, order_time) = book.selected();
    let benchmark_mid = benchmark.and_then(|b| b["price"].as_str()).map(decimal);
    let benchmark_time = benchmark
        .map(|b| b["observed_at"].clone())
        .unwrap_or(Json::Null);
    let mut value = None;
    let mut signal = "NO_DATA";
    let mut source = "NO_DATA";
    let mut label = "No data";
    let mut observed = Json::Null;
    let mut executable = false;
    let mut reference = false;
    let mut policy = json!({"public_label":"No data","tooltip":"No eligible public signal is available for this market slice.","allowed_terms":["no data"],"forbidden_terms":["live","firm","executable","market price","clearing"],"disclaimer":null});
    let managed = book.demo_count > 0
        && book.demo_count == book.managed_count
        && book.status() == "DEMO_ONLY"
        && book.demo_bid.is_some()
        && book.demo_ask.is_some()
        && trade.is_none_or(|t| t["demo_status"] == "DEMO_ONLY");
    let indication_mid = indication["latest_mid_price_per_mt_usd"]
        .as_str()
        .map(decimal)
        .or_else(|| {
            let bid = indication["latest_bid_price_per_mt_usd"]
                .as_str()
                .map(decimal);
            let ask = indication["latest_ask_price_per_mt_usd"]
                .as_str()
                .map(decimal);
            midpoint(bid, ask)
        });
    if let Some(trade) = trade.filter(|_| !managed) {
        value = trade["price_per_mt_usd"].as_str().map(decimal);
        signal = "CONFIRMED_TRADE";
        source = if trade["demo_status"] == "DEMO_ONLY" {
            "DEMO_SEED"
        } else {
            "CONFIRMED_TRADE"
        };
        label = "Historical confirmed trade";
        observed = trade["confirmed_at"].clone();
        policy = label_policy(
            label,
            "Latest anonymized confirmed trade print for this exact product, port, and window.",
            &["historical", "confirmed trade"],
            None,
        );
    } else if let Some(mid) = midpoint(book.real_bid, book.real_ask) {
        value = Some(mid);
        signal = "ORDERBOOK_BID";
        source = "LIVE_ORDER";
        label = "Executable orderbook midpoint";
        observed = order_time.map(timestamp).unwrap_or(Json::Null);
        executable = true;
        policy = label_policy(
            label,
            "Midpoint of the best visible bid and ask for this exact slice. One-sided books do not create a midpoint.",
            &["orderbook", "midpoint", "executable"],
            None,
        );
    } else if book.status() == "DEMO_ONLY" && midpoint(book.demo_bid, book.demo_ask).is_some() {
        value = midpoint(book.demo_bid, book.demo_ask);
        signal = "ORDERBOOK_BID";
        source = "DEMO_SEED";
        label = "Demo orderbook midpoint";
        observed = order_time.map(timestamp).unwrap_or(Json::Null);
        policy = label_policy(
            label,
            "Demo midpoint of the best visible bid and ask for this exact slice. Not executable user liquidity.",
            &["demo", "orderbook", "midpoint"],
            None,
        );
    } else if let Some(mid) = indication_mid {
        value = Some(mid);
        signal = "MARKET_INDICATION";
        source = if indication["provenance"]["demo_status"] == "DEMO_ONLY" {
            "DEMO_SEED"
        } else {
            "UNKNOWN"
        };
        label = "Market indication";
        observed = indication["provenance"]["observed_at"].clone();
        reference = true;
        policy = label_policy(
            label,
            "Non-executable market indication for this exact slice.",
            &["indication", "non-executable"],
            None,
        );
    } else if let Some(mid) = band["mid_price_per_mt_usd"].as_str() {
        value = Some(decimal(mid));
        signal = "FAIR_PRICE_BAND";
        source = "BENCHMARK_REFERENCE";
        label = "Verdaxis fair band";
        observed = band["provenance"]["observed_at"].clone();
        reference = true;
        policy = label_policy(
            label,
            "Modelled fair-value band for this exact slice.",
            &["fair band", "modelled"],
            Some(DISCLAIMER),
        );
    } else if let Some(mid) = benchmark_mid {
        value = Some(mid);
        signal = "BENCHMARK_MID";
        source = "BENCHMARK_REFERENCE";
        label = "Benchmark reference";
        observed = benchmark_time.clone();
        reference = true;
        policy = label_policy(
            label,
            "Persisted benchmark reference for this exact slice.",
            &["benchmark", "reference"],
            None,
        );
    }
    let profile_policy = &super::super::benchmarks::policy()["reference_profiles"][profile];
    if !profile_policy.is_null() {
        policy["disclaimer"] = profile_policy["disclaimer"].clone();
        if executable {
            label = "Orderbook midpoint";
            executable = false;
            reference = true;
            policy["public_label"] = json!(label);
            policy["allowed_terms"] = json!(["orderbook", "midpoint"]);
        }
    }
    let freshness = observed
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|date| {
            if date.with_timezone(&Utc) >= now - Duration::days(7) {
                "FRESH"
            } else {
                "STALE"
            }
        })
        .unwrap_or("NO_DATA");
    (
        json!({"primary_value":value.map(money),"primary_signal_type":signal,"primary_source_kind":source,"public_source_label":label,
        "staleness_status":freshness,"is_executable":executable,"is_reference":reference,
        "demo_status":trade.map(|t|t["demo_status"].clone()).unwrap_or_else(||json!(book.status())),"observed_at":observed,
        "best_bid":bid.map(money),"best_ask":ask.map(money),"spread":bid.zip(ask).map(|(b,a)|money(a-b)),
        "volume_mt":if volume==Decimal::ZERO {"0".to_owned()}else{money(volume)},"order_count":count,
        "real_order_count":book.real_count,"demo_order_count":book.demo_count,"unknown_order_count":book.unknown_count,
        "real_best_bid":book.real_bid.map(money),"real_best_ask":book.real_ask.map(money),"demo_best_bid":book.demo_bid.map(money),"demo_best_ask":book.demo_ask.map(money),
        "benchmark_mid":benchmark_mid.map(money),"benchmark_source_kind":if benchmark.is_some(){"BENCHMARK_REFERENCE"}else{"NO_DATA"},"benchmark_observed_at":benchmark_time}),
        policy,
    )
}
