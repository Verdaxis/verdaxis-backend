use super::*;
use curve_projection::{DISCLAIMER, no_band};
use rust_decimal::Decimal;

fn focus_point_matches(value: &Json, id: &str) -> bool {
    value
        .as_str()
        .and_then(|text| uuid::Uuid::parse_str(text).ok())
        .is_some_and(|value| Some(value) == uuid::Uuid::parse_str(id).ok())
}

fn sort_latest_signals(latest: &mut [Json]) {
    latest.sort_by_cached_key(|signal| {
        std::cmp::Reverse(
            DateTime::parse_from_rfc3339(
                signal["observed_at"]
                    .as_str()
                    .expect("observed signal timestamp"),
            )
            .expect("serialized UTC timestamp"),
        )
    });
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    #[test]
    fn board_focus_accepts_uuid_spellings() {
        let id = "1379d36c-1ca9-55b7-9c0d-5235a0ba1f36";
        for spelling in [
            id.to_owned(),
            id.to_uppercase(),
            id.replace('-', ""),
            format!("{{{id}}}"),
            format!("urn:uuid:{id}"),
        ] {
            assert!(focus_point_matches(&json!(spelling), id), "{spelling}");
        }
        assert!(!focus_point_matches(&Json::Null, id));
    }

    #[test]
    fn latest_signals_use_subsecond_instants_and_stable_ties() {
        let mut signals = vec![
            json!({"id": "older", "observed_at": "2026-10-15T12:00:00Z"}),
            json!({"id": "newer", "observed_at": "2026-10-15T12:00:00.500000Z"}),
            json!({"id": "tie", "observed_at": "2026-10-15T12:00:00.500000Z"}),
        ];
        sort_latest_signals(&mut signals);
        assert_eq!(
            signals
                .iter()
                .map(|v| v["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["newer", "tie", "older"]
        );
    }
}

fn cell_values(
    product: &Product,
    key: &SliceKey,
    inputs: &Inputs,
    now: DateTime<Utc>,
) -> (Json, Json) {
    let empty = Book::default();
    curve_projection::cells::values(
        inputs.books.get(key).unwrap_or(&empty),
        inputs.trades.get(key).and_then(|v| v.first()),
        inputs.benchmarks.get(key),
        &inputs.signals.indication_summary(key),
        &inputs.signals.band(key),
        &product.profile,
        now,
    )
}
pub(super) fn table_projection(
    products: &[Product],
    points: &[Point],
    windows: &[String],
    keys: &[SliceKey],
    inputs: &Inputs,
    now: DateTime<Utc>,
) -> Result<Json, ApiError> {
    let mut rows = Vec::new();
    let mut latest = Vec::new();
    for product in products {
        for point in points
            .iter()
            .filter(|point| product.lanes.contains(&point.id))
        {
            let mut cells = serde_json::Map::new();
            for window in windows {
                let key = key(product, point, window);
                let (mut cell, _) = cell_values(product, &key, inputs, now);
                if !cell["primary_value"].is_null() && !cell["observed_at"].is_null() {
                    latest.push(json!({"market_product":product.code,"delivery_point_id":point.id,"delivery_point_name":point.name,"availability_window":window,
                    "primary_value":cell["primary_value"],"primary_signal_type":cell["primary_signal_type"],"primary_source_kind":cell["primary_source_kind"],"public_source_label":cell["public_source_label"],"demo_status":cell["demo_status"],"observed_at":cell["observed_at"],"staleness_status":cell["staleness_status"]}));
                }
                for field in [
                    "real_best_bid",
                    "real_best_ask",
                    "demo_best_bid",
                    "demo_best_ask",
                    "benchmark_mid",
                    "benchmark_source_kind",
                    "benchmark_observed_at",
                ] {
                    cell.as_object_mut().unwrap().shift_remove(field);
                }
                cells.insert(window.clone(), cell);
            }
            rows.push(json!({"row_key":format!("{}:{}",product.code,point.id),"market_product":product.code,"product_name":product.name,
            "representative_product_id":product.id,"product_count":1,"delivery_point_id":point.id,"delivery_point_name":point.name,"region":point.region,"cells":cells}));
        }
    }
    sort_latest_signals(&mut latest);
    latest.truncate(8);
    let columns:Result<Vec<_>,ApiError>=windows.iter().map(|w|Ok(json!({"availability_window":w,"display_label":window_display_label(w).map_err(invalid)?,"group":if w=="SPOT"{"Spot"}else if w.ends_with("-CAL"){"Calendar"}else if w.contains("-Q"){"Quarterly"}else{"Monthly"}}))).collect();
    let _ = keys;
    Ok(
        json!({"columns":columns?,"rows":rows,"latest_signals":latest,"generated_at":timestamp(now),"disclaimer":DISCLAIMER}),
    )
}
fn full_cell(
    product: &Product,
    point: &Point,
    key: &SliceKey,
    inputs: &Inputs,
    now: DateTime<Utc>,
) -> Json {
    let (values, policy) = cell_values(product, key, inputs, now);
    let band = inputs.signals.band(key);
    let mut cell = json!({"market_product":product.code,"product_name":product.name,"representative_product_id":product.id,"product_count":1,
        "delivery_point_id":point.id,"delivery_point_name":point.name,"region":point.region,"availability_window":key.2});
    for (name, value) in values.as_object().unwrap() {
        cell[name] = value.clone();
        if name == "public_source_label" {
            cell["label_policy"] = policy.clone();
        }
        if name == "demo_status" {
            cell["scope"] = json!("DELIVERY_POINT");
        }
        if name == "observed_at" {
            cell["generated_at"] = timestamp(now);
        }
    }
    cell["indication_summary"] = inputs.signals.indication_summary(key);
    cell["fair_price_band"] = band.clone();
    cell["fair_price_band_provenance"] = if band.is_null() {
        no_band(now)
    } else {
        band["provenance"].clone()
    };
    cell["physical_stem_summary"] = inputs.signals.stem_summary(key);
    cell
}
fn depth(product: &Product, key: &SliceKey, inputs: &Inputs) -> (Vec<Json>, Vec<Json>) {
    let rows: Vec<_> = inputs
        .orders
        .iter()
        .filter(|row| {
            row.get::<&str, _>("product_id") == product.id
                && row.get::<&str, _>("delivery_point_id") == key.1
                && row.get::<&str, _>("availability_window") == key.2
        })
        .collect();
    let real = rows
        .iter()
        .any(|r| r.get::<&str, _>("provenance") == "REAL");
    let mut levels: HashMap<(String, Decimal), (Decimal, i64)> = HashMap::new();
    for row in rows {
        if (row.get::<&str, _>("provenance") == "REAL") != real {
            continue;
        }
        let level = levels
            .entry((row.get("side"), decimal(row.get("price"))))
            .or_default();
        level.0 += decimal(row.get("quantity"));
        level.1 += 1;
    }
    let mut bids = Vec::new();
    let mut asks = Vec::new();
    for ((side, price), (quantity, count)) in levels {
        let value = json!({"price_per_mt_usd":money(price),"quantity_mt":money(quantity),"order_count":count,"source_kind":if real{"LIVE_ORDER"}else{"DEMO_SEED"},"demo_status":if real{"REAL_ONLY"}else{"DEMO_ONLY"},"real_order_count":if real{count}else{0},"demo_order_count":if real{0}else{count},"unknown_order_count":0});
        if side == "BID" {
            bids.push((price, value));
        } else {
            asks.push((price, value));
        }
    }
    bids.sort_by_key(|level| std::cmp::Reverse(level.0));
    asks.sort_by_key(|a| a.0);
    (
        bids.into_iter().take(10).map(|(_, v)| v).collect(),
        asks.into_iter().take(10).map(|(_, v)| v).collect(),
    )
}
#[allow(clippy::too_many_arguments)]
fn evidence(
    layer: &str,
    side: Json,
    price: Json,
    low: Json,
    high: Json,
    quantity: Json,
    observed: Json,
    label: &str,
    source: Json,
    demo: Json,
) -> Json {
    json!({"layer":layer,"side":side,"price_per_mt_usd":price,"low_price_per_mt_usd":low,"high_price_per_mt_usd":high,"quantity_mt":quantity,"observed_at":observed,"public_source_label":label,"source_kind":source,"demo_status":demo})
}
pub(super) fn slice_projection(
    product: &Product,
    point: &Point,
    key: &SliceKey,
    inputs: &Inputs,
    now: DateTime<Utc>,
) -> Result<Json, ApiError> {
    let cell = full_cell(product, point, key, inputs, now);
    let (bids, asks) = depth(product, key, inputs);
    let trades = inputs.trades.get(key).cloned().unwrap_or_default();
    let indications = inputs.signals.focus_indications(key);
    let stems = inputs.signals.focus_stems(key, 6);
    let band = inputs.signals.band(key);
    let windows = default_windows(now.date_naive(), false);
    let index = windows.iter().position(|w| w == &key.2);
    let previous = index
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| windows.get(i));
    let next = index.and_then(|i| windows.get(i + 1));
    let mut evidence_points = Vec::new();
    for trade in &trades {
        evidence_points.push(evidence(
            "HISTORICAL_TRADE",
            Json::Null,
            trade["price_per_mt_usd"].clone(),
            Json::Null,
            Json::Null,
            trade["quantity_mt"].clone(),
            trade["confirmed_at"].clone(),
            "Historical confirmed trade",
            trade["source_kind"].clone(),
            trade["demo_status"].clone(),
        ));
    }
    for (levels, layer, side, label) in [
        (&bids, "ORDERBOOK_BID", "BID", "Visible bid depth"),
        (&asks, "ORDERBOOK_ASK", "ASK", "Visible ask depth"),
    ] {
        for level in levels {
            evidence_points.push(evidence(
                layer,
                json!(side),
                level["price_per_mt_usd"].clone(),
                Json::Null,
                Json::Null,
                level["quantity_mt"].clone(),
                cell["observed_at"].clone(),
                label,
                level["source_kind"].clone(),
                level["demo_status"].clone(),
            ));
        }
    }
    for indication in &indications {
        evidence_points.push(evidence(
            "MARKET_INDICATION",
            indication["side"].clone(),
            indication["price_per_mt_usd"].clone(),
            Json::Null,
            Json::Null,
            indication["quantity_mt"].clone(),
            indication["provenance"]["observed_at"].clone(),
            "Market indication",
            json!(if indication["provenance"]["demo_status"] == "DEMO_ONLY" {
                "DEMO_SEED"
            } else {
                "UNKNOWN"
            }),
            indication["provenance"]["demo_status"].clone(),
        ));
    }
    if !band["mid_price_per_mt_usd"].is_null() {
        evidence_points.push(evidence(
            "FAIR_PRICE_BAND",
            Json::Null,
            band["mid_price_per_mt_usd"].clone(),
            band["low_price_per_mt_usd"].clone(),
            band["high_price_per_mt_usd"].clone(),
            Json::Null,
            band["provenance"]["observed_at"].clone(),
            "Verdaxis fair band",
            json!("BENCHMARK_REFERENCE"),
            band["provenance"]["demo_status"].clone(),
        ));
    }
    if !cell["benchmark_mid"].is_null() {
        evidence_points.push(evidence(
            "BENCHMARK_MID",
            Json::Null,
            cell["benchmark_mid"].clone(),
            Json::Null,
            Json::Null,
            Json::Null,
            cell["benchmark_observed_at"].clone(),
            "Benchmark reference",
            cell["benchmark_source_kind"].clone(),
            json!("NOT_APPLICABLE"),
        ));
    }
    for stem in &stems {
        evidence_points.push(evidence(
            "PHYSICAL_STEM",
            Json::Null,
            Json::Null,
            Json::Null,
            Json::Null,
            stem["quantity_mt"].clone(),
            stem["provenance"]["observed_at"].clone(),
            if stem["status"] == "AVAILABLE" {
                "Available physical stem"
            } else {
                "Physical stem signal"
            },
            json!(if stem["provenance"]["demo_status"] == "DEMO_ONLY" {
                "DEMO_SEED"
            } else {
                "UNKNOWN"
            }),
            stem["provenance"]["demo_status"].clone(),
        ));
    }
    Ok(
        json!({"cell":cell,"previous_window":previous,"next_window":next,"depth_bids":bids,"depth_asks":asks,"trades":trades,"indications":indications,"fair_price_band":band,"physical_stems":stems,"evidence_points":evidence_points,"generated_at":timestamp(now),"disclaimer":DISCLAIMER}),
    )
}

fn board_cell(
    product: &Product,
    point: &Point,
    key: &SliceKey,
    inputs: &Inputs,
    now: DateTime<Utc>,
) -> Json {
    let empty = Book::default();
    let book = inputs.books.get(key).unwrap_or(&empty);
    let (bid, ask, volume, count, observed) = book.selected();
    let mut quote = benchmarks::board_quote(inputs.benchmarks.get(key), key, now);
    let demo = quote["source"].as_str().is_none_or(|s| s == "seed_matrix");
    if demo && book.real_count > 0 {
        quote = Json::Null;
    }
    let has_quote = !quote.is_null();
    let band = inputs.signals.band(key);
    json!({"product_id":product.id,"market_product":product.code,"product_name":product.name,"delivery_point_id":point.id,"delivery_point_name":point.name,"region":point.region,"availability_window":key.2,
        "benchmark_mid":quote["price"].as_str().map(|s|money(decimal(s))),"benchmark_source":quote["source"],"is_demo_benchmark":has_quote&&demo,
        "order_source_kind":book.source(),"benchmark_source_kind":if !has_quote{"NO_DATA"}else if demo{"DEMO_SEED"}else{"BENCHMARK_REFERENCE"},"scope":"DELIVERY_POINT","demo_status":book.status(),
        "real_order_count":book.real_count,"demo_order_count":book.demo_count,"unknown_order_count":book.unknown_count,
        "real_best_bid":book.real_bid.map(money),"real_best_ask":book.real_ask.map(money),"demo_best_bid":book.demo_bid.map(money),"demo_best_ask":book.demo_ask.map(money),
        "best_bid_source_kind":if bid.is_some(){book.source()}else{"NO_DATA"},"best_ask_source_kind":if ask.is_some(){book.source()}else{"NO_DATA"},"order_observed_at":observed.map(timestamp),"benchmark_observed_at":quote["observed_at"],
        "indication_summary":inputs.signals.indication_summary(key),"fair_price_band":band,"fair_price_band_provenance":if band.is_null(){no_band(now)}else{band["provenance"].clone()},"physical_stem_summary":inputs.signals.stem_summary(key),
        "best_bid":bid.map(money),"best_ask":ask.map(money),"spread":bid.zip(ask).map(|(b,a)|money(a-b)),"volume_mt":if volume==Decimal::ZERO{"0".to_owned()}else{money(volume)},"order_count":count})
}
pub(super) async fn load_board(
    connection: &mut PgConnection,
    state: &AppState,
    query: &Json,
    now: DateTime<Utc>,
    products: Vec<Product>,
    points: Vec<Point>,
) -> Result<Json, ApiError> {
    let window = normalize_window(
        query["availability_window"].as_str().unwrap_or("SPOT"),
        now.date_naive(),
    )
    .map_err(invalid)?;
    let product = products
        .iter()
        .find(|p| {
            query
                .get("focus_market_product")
                .unwrap_or(&benchmarks::policy()["default_focus_product"])
                == &p.code
        })
        .or_else(|| products.first())
        .ok_or_else(|| {
            invalid("Forward curve board requires active products and delivery points")
        })?;
    let point = points
        .iter()
        .find(|p| focus_point_matches(&query["focus_delivery_point_id"], &p.id))
        .or_else(|| {
            points
                .iter()
                .find(|p| benchmarks::policy()["default_focus_point"] == p.id)
        })
        .or_else(|| points.first())
        .ok_or_else(|| {
            invalid("Forward curve board requires active products and delivery points")
        })?;
    if !product.lanes.contains(&point.id) {
        return Err(invalid(
            "market_product is not available at this delivery point",
        ));
    }
    let mut windows = default_windows(now.date_naive(), true);
    let mut keys: Vec<_> = points
        .iter()
        .flat_map(|point| {
            products
                .iter()
                .filter(|p| p.lanes.contains(&point.id))
                .map(|p| key(p, point, &window))
        })
        .collect();
    keys.extend(windows.iter().map(|w| key(product, point, w)));
    let mut seen = HashSet::new();
    keys.retain(|k| seen.insert(k.clone()));
    let inputs = inputs(
        connection,
        state,
        &products,
        &keys,
        now,
        false,
        Some((product, point)),
    )
    .await?;
    for key in inputs
        .books
        .keys()
        .filter(|k| k.0 == product.code && k.1 == point.id)
    {
        if !windows.contains(&key.2) {
            windows.push(key.2.clone());
        }
    }
    let mut sorted = Vec::new();
    for window in windows {
        sorted.push((
            window_sort_key(&window, now.date_naive()).map_err(invalid)?,
            window,
        ));
    }
    sorted.sort_by_key(|(k, _)| *k);
    let curve: Vec<_> = sorted
        .iter()
        .map(|(_, w)| board_cell(product, point, &key(product, point, w), &inputs, now))
        .collect();
    let ports:Vec<_>=points.iter().map(|point|json!({"delivery_point_id":point.id,"delivery_point_name":point.name,"region":point.region,"cells":products.iter().filter(|p|p.lanes.contains(&point.id)).map(|p|board_cell(p,point,&key(p,point,&window),&inputs,now)).collect::<Vec<_>>()})).collect();
    let key = key(product, point, &window);
    let (bids, asks) = depth(product, &key, &inputs);
    let band = inputs.signals.band(&key);
    let focus = json!({"product_id":product.id,"market_product":product.code,"product_name":product.name,"delivery_point_id":point.id,"delivery_point_name":point.name,"region":point.region,"availability_window":window,
        "curve":curve,"depth_bids":bids,"depth_asks":asks,"indications":inputs.signals.focus_indications(&key),"fair_price_band":band,"fair_price_band_provenance":if band.is_null(){no_band(now)}else{band["provenance"].clone()},"physical_stems":inputs.signals.focus_stems(&key,10)});
    Ok(
        json!({"availability_window":window,"products":products.iter().map(|p|json!({"product_id":p.id,"market_product":p.code,"product_name":p.name})).collect::<Vec<_>>(),"ports":ports,"focus":focus,"generated_at":timestamp(now)}),
    )
}
