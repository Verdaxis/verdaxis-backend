//! Pure voyage calculations. Decimal operation order follows the Python oracle.
use crate::Json;
use crate::market::amount::Amount as D;
use crate::market::fueleu;
use chrono::{Datelike, Duration, NaiveDate};
use serde::Deserialize;
use serde_json::json;
use std::cmp::Ordering;

fn d(text: &str) -> D {
    D::parse(text).expect("reviewed decimal constant")
}
fn number(value: &Json) -> Result<D, String> {
    D::parse(
        value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string())
            .as_str(),
    )
}
fn rounded(value: Option<&D>, scale: u32) -> Result<Json, String> {
    value
        .map(|value| value.quantize(scale).map(Json::String))
        .transpose()
        .map(|value| value.unwrap_or(Json::Null))
}
fn positive(value: &D) -> bool {
    value.compare(&d("0")) == Ordering::Greater
}
fn unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}
fn properties() -> &'static Json {
    static DATA: std::sync::LazyLock<Json> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!(
            "../../../../deploy/rust/voyage-properties.json"
        ))
        .expect("voyage properties")
    });
    &DATA
}
fn catalog() -> &'static Json {
    static DATA: std::sync::LazyLock<Json> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../../../../deploy/rust/mcp-catalog.json"))
            .expect("catalog data")
    });
    &DATA
}

fn available(fuel: &str, port: &str) -> bool {
    catalog()["products"]
        .as_array()
        .unwrap()
        .iter()
        .any(|product| {
            product["code"] == fuel
                && product["delivery_points"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|point| point == port)
        })
}

pub(super) fn canonical_port(port: &str) -> Result<String, String> {
    // Catalog names are ASCII. Long s is the additional Unicode case-fold
    // equivalence that can occur in these names (Python uses str.casefold).
    let folded: String = port
        .trim_matches(crate::market::whitespace::is_python_whitespace)
        .chars()
        .flat_map(|character| {
            if character == '\u{017f}' {
                's'
            } else {
                character
            }
            .to_lowercase()
        })
        .collect();
    let ports: Vec<_> = catalog()["delivery_points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|point| point["name"].as_str().unwrap())
        .collect();
    ports
        .iter()
        .find(|name| name.to_lowercase() == folded)
        .map(|name| (*name).to_owned())
        .ok_or_else(|| {
            format!(
                "Unsupported port {}. Supported ports: {}",
                super::transport::python_string_repr(port),
                ports.join(", ")
            )
        })
}

fn parse_request(input: &Json) -> Result<Request, String> {
    let mut arguments = json!({"request": input});
    super::validation::tool_arguments("plan_voyage_fuel", &mut arguments)?;
    serde_json::from_value(arguments["request"].clone()).map_err(|error| error.to_string())
}

fn conventional() -> &'static str {
    properties()["conventional_fuel"]
        .as_str()
        .expect("conventional identity")
}
fn eu(port: &str) -> bool {
    properties()["eu_ports"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == port)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Vessel {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    imo_number: Option<String>,
    speed_knots: Json,
    sea_consumption_mt_per_day: Json,
    #[serde(default = "zero")]
    berth_consumption_mt_per_day: Json,
    compatible_fuels: Vec<String>,
}
fn zero() -> Json {
    json!("0")
}
fn default_route() -> String {
    "DEFAULT".into()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Leg {
    from_port: String,
    to_port: String,
    #[serde(default = "default_route")]
    route: String,
    #[serde(default)]
    distance_nm: Option<Json>,
    #[serde(default = "zero")]
    berth_days_at_destination: Json,
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Assumptions {
    #[serde(default)]
    vlsfo_price_usd_per_mt: serde_json::Map<String, Json>,
    #[serde(default)]
    eua_price_eur_per_tco2: Option<Json>,
    #[serde(default)]
    fueleu_pool_price_eur_per_tco2e: Option<Json>,
    #[serde(default)]
    usd_per_eur: Option<Json>,
    #[serde(default)]
    ghg_intensity_gco2e_per_mj: serde_json::Map<String, Json>,
    #[serde(default)]
    reporting_year: Option<i32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    vessel: Vessel,
    legs: Vec<Leg>,
    start_date: NaiveDate,
    #[serde(default)]
    assumptions: Assumptions,
}
#[derive(Clone)]
struct Segment {
    index: usize,
    kind: &'static str,
    port: String,
    departure: NaiveDate,
    energy: D,
    scope: D,
}
#[derive(Clone)]
struct Choice {
    intensity: Option<D>,
    source: Option<&'static str>,
    price: Option<Json>,
    lcv: D,
}
struct Burn {
    segment: Segment,
    fuel: String,
    energy: D,
    choice: Choice,
}

fn legs(request: &Request) -> Result<(Vec<Json>, Vec<Segment>), String> {
    let data: Json =
        serde_json::from_str(include_str!("../../../../deploy/rust/sea-distances.json")).unwrap();
    let mut departure = request.start_date;
    let mut output = Vec::new();
    let mut segments = Vec::new();
    for (index, leg) in request.legs.iter().enumerate() {
        let table = data["routes"].as_array().unwrap().iter().find(|row| {
            row["route"] == leg.route
                && ((row["from_port"] == leg.from_port && row["to_port"] == leg.to_port)
                    || (row["from_port"] == leg.to_port && row["to_port"] == leg.from_port))
        });
        if leg.route == "CAPE_OF_GOOD_HOPE" && table.is_none() {
            return Err(format!(
                "No Cape route for {} to {}",
                leg.from_port, leg.to_port
            ));
        }
        let distance = match &leg.distance_nm {
            Some(value) => number(value)?,
            None => number(&table.ok_or("No distance")?["distance_nm"])?,
        };
        let days = distance.divide(&number(&request.vessel.speed_knots)?.multiply(&d("24")))?;
        let berth = number(&leg.berth_days_at_destination)?;
        let sea_calendar = days.ceil_days()?;
        let berth_calendar = berth.ceil_days()?;
        if (departure - request.start_date).num_days() + sea_calendar + berth_calendar > 365 {
            return Err(
                "The voyage is longer than 365 days; check distances, speed and berth days.".into(),
            );
        }
        let arrival = departure + Duration::days(sea_calendar);
        let scope = d(&(u8::from(eu(&leg.from_port)) + u8::from(eu(&leg.to_port))).to_string())
            .divide(&d("2"))?;
        let berth_scope = d(if eu(&leg.to_port) { "1" } else { "0" });
        let energy = days
            .multiply(&number(&request.vessel.sea_consumption_mt_per_day)?)
            .multiply(&d("41000"));
        let berth_energy = berth
            .multiply(&number(&request.vessel.berth_consumption_mt_per_day)?)
            .multiply(&d("41000"));
        for (kind, energy, scope) in [
            ("SEA", energy.clone(), scope.clone()),
            ("BERTH", berth_energy.clone(), berth_scope.clone()),
        ] {
            segments.push(Segment {
                index,
                kind,
                port: leg.from_port.clone(),
                departure,
                energy,
                scope,
            });
        }
        output.push(json!({"from_port":leg.from_port,"to_port":leg.to_port,"route":leg.route,
            "via":table.map(|row|row["via"].clone()).unwrap_or(json!("Caller distance")),"distance_nm":distance.text(),
            "distance_source":if leg.distance_nm.is_some(){"CALLER"}else{"VERDAXIS_TABLE"},
            "departure_date":departure.to_string(),"arrival_date":arrival.to_string(),"sea_days":days.quantize(4)?,
            "eu_scope_share":scope.quantize(4)?,"sea_energy_mj":energy.quantize(0)?,"berth_days":berth.quantize(4)?,
            "berth_eu_scope_share":berth_scope.quantize(4)?,"berth_energy_mj":berth_energy.quantize(0)?}));
        departure = arrival + Duration::days(berth_calendar);
    }
    Ok((output, segments))
}

fn choice(
    request: &Request,
    prices: &Json,
    segment: &Segment,
    fuel: &str,
) -> Result<Choice, String> {
    let property = &properties()["fuels"][fuel];
    let price = if fuel == conventional() {
        request.assumptions.vlsfo_price_usd_per_mt.get(&segment.port).map(|amount| json!({"fuel":fuel,"port":segment.port,"price_usd_per_mt":amount,"source":"CALLER","executable":false,"availability_window":null,"observed_at":null,"carbon_intensity_gco2e_per_mj":null,"energy_density_mj_per_kg":null}))
    } else if !available(fuel, &segment.port) {
        None
    } else {
        prices
            .as_array()
            .ok_or("prices must be an array")?
            .iter()
            .find(|entry| {
                entry["lookup"]["fuel"] == fuel
                    && entry["lookup"]["port"] == segment.port
                    && entry["lookup"]["departure_date"] == segment.departure.to_string()
            })
            .map(|entry| entry["price"].clone())
    };
    let (intensity, source) =
        if let Some(value) = request.assumptions.ghg_intensity_gco2e_per_mj.get(fuel) {
            (Some(number(value)?), Some("CALLER"))
        } else if let Some(value) = price
            .as_ref()
            .and_then(|price| price.get("carbon_intensity_gco2e_per_mj"))
            .filter(|value| !value.is_null())
        {
            (Some(number(value)?), Some("ORDER_DECLARED"))
        } else if property["default_intensity_gco2e_per_mj"].is_null() {
            (None, None)
        } else {
            (
                Some(number(&property["default_intensity_gco2e_per_mj"])?),
                Some("DEFAULT_ILLUSTRATIVE"),
            )
        };
    let lcv = price
        .as_ref()
        .and_then(|price| price.get("energy_density_mj_per_kg"))
        .filter(|value| !value.is_null())
        .unwrap_or(&property["lcv_mj_per_kg"]);
    Ok(Choice {
        intensity,
        source,
        price: price.clone(),
        lcv: number(lcv)?,
    })
}

struct StrategySpec<'a> {
    id: &'a str,
    green: Option<&'a str>,
    minimum: bool,
    basis: Option<&'a str>,
}

fn strategy(
    request: &Request,
    prices: &Json,
    segments: &[Segment],
    target: &D,
    spec: StrategySpec<'_>,
    warnings: &mut Vec<String>,
) -> Result<Json, String> {
    let StrategySpec {
        id,
        green,
        minimum,
        basis,
    } = spec;
    let has_conventional = request
        .vessel
        .compatible_fuels
        .iter()
        .any(|fuel| fuel == conventional());
    let mut missing = Vec::new();
    let mut burns = Vec::new();
    let mut unknown_blend = false;
    for segment in segments {
        if segment.energy.compare(&d("0")) == Ordering::Equal {
            continue;
        }
        let mut share = d("0");
        if let Some(fuel) = green.filter(|_| positive(&segment.scope) || !has_conventional) {
            let details = choice(request, prices, segment, fuel)?;
            share = d("1");
            if minimum {
                let conventional = choice(request, prices, segment, conventional())?
                    .intensity
                    .ok_or("missing conventional intensity")?;
                let Some(intensity) = &details.intensity else {
                    unique(&mut missing, format!("ghg_intensity_gco2e_per_mj[{fuel}]"));
                    unknown_blend = true;
                    continue;
                };
                share = conventional
                    .subtract(target)
                    .divide(&conventional.subtract(intensity))?;
            }
            burns.push(Burn {
                segment: segment.clone(),
                fuel: fuel.into(),
                energy: segment.energy.multiply(&share),
                choice: details,
            });
        }
        if share.compare(&d("1")) == Ordering::Less {
            burns.push(Burn {
                segment: segment.clone(),
                fuel: conventional().into(),
                energy: segment.energy.multiply(&d("1").subtract(&share)),
                choice: choice(request, prices, segment, conventional())?,
            });
        }
    }
    let mut fuel_cost = Some(d("0"));
    let mut co2 = d("0");
    let mut uses = Vec::new();
    let mut weighted = d("0");
    let mut unknown_intensity = unknown_blend;
    let mut sources = serde_json::Map::new();
    for burn in burns {
        let tonnes = burn.energy.divide(&burn.choice.lcv.multiply(&d("1000")))?;
        co2 = co2.add(&burn.segment.scope.multiply(&tonnes).multiply(&number(
            &properties()["fuels"][&burn.fuel]["ets_co2_t_per_t"],
        )?));
        let mut output_price = burn.choice.price.clone();
        let cost = if let Some(price) = &burn.choice.price {
            let cost = tonnes.multiply(&number(&price["price_usd_per_mt"])?);
            fuel_cost = fuel_cost.map(|total| total.add(&cost));
            if price["source"] == "DEMO_ORDER" {
                unique(
                    warnings,
                    "Used price is demo preview liquidity, not executable liquidity.".into(),
                );
            }
            if price["source"] == "BENCHMARK" {
                unique(
                    warnings,
                    "Used benchmark price is an indicative reference, not an executable quote."
                        .into(),
                );
            }
            let output = output_price.as_mut().unwrap();
            output["price_usd_per_mt"] = rounded(Some(&number(&price["price_usd_per_mt"])?), 2)?;
            if !price["carbon_intensity_gco2e_per_mj"].is_null() {
                output["carbon_intensity_gco2e_per_mj"] =
                    rounded(Some(&number(&price["carbon_intensity_gco2e_per_mj"])?), 4)?;
            }
            Some(cost)
        } else {
            unique(
                &mut missing,
                if burn.fuel == conventional() {
                    format!("vlsfo_price_usd_per_mt[{}]", burn.segment.port)
                } else {
                    format!("market_price[{}@{}]", burn.fuel, burn.segment.port)
                },
            );
            fuel_cost = None;
            None
        };
        uses.push(json!({"leg_index":burn.segment.index,"segment":burn.segment.kind,"fuel":burn.fuel,"tonnes":tonnes.quantize(2)?,"bunkering_port":burn.segment.port,"price":output_price,"cost_usd":rounded(cost.as_ref(),2)?}));
        if let Some(source) = burn.choice.source {
            sources.insert(burn.fuel.clone(), json!(source));
        }
        if positive(&burn.segment.scope) {
            if let Some(intensity) = burn.choice.intensity {
                weighted = weighted.add(
                    &burn
                        .segment
                        .scope
                        .multiply(&burn.energy)
                        .multiply(&intensity),
                );
            } else {
                unknown_intensity = true;
                unique(
                    &mut missing,
                    format!("ghg_intensity_gco2e_per_mj[{}]", burn.fuel),
                );
            }
        }
    }
    let energy = segments.iter().fold(d("0"), |sum, segment| {
        sum.add(&segment.scope.multiply(&segment.energy))
    });
    let intensity = if unknown_intensity || !positive(&energy) {
        None
    } else if minimum {
        Some(target.clone())
    } else {
        Some(weighted.divide(&energy)?)
    };
    let balance = if unknown_intensity {
        None
    } else if !positive(&energy) {
        Some(d("0"))
    } else {
        Some(
            target
                .subtract(intensity.as_ref().unwrap())
                .multiply(&energy)
                .divide(&d("1000000"))?,
        )
    };
    let pool = request
        .assumptions
        .fueleu_pool_price_eur_per_tco2e
        .as_ref()
        .map(number)
        .transpose()?;
    let (fueleu_cost, cost_basis) = match balance.as_ref() {
        None => (None, "NONE"),
        Some(balance) if balance.compare(&d("0")) == Ordering::Equal => (Some(d("0")), "NONE"),
        Some(balance) if positive(balance) => match &pool {
            Some(pool) => (
                Some(d("0").subtract(balance).multiply(pool)),
                "SURPLUS_AT_POOL_PRICE",
            ),
            None => {
                unique(
                    warnings,
                    format!(
                        "FuelEU surplus of {} tCO2e in {id} is not valued; supply fueleu_pool_price_eur_per_tco2e to value it.",
                        balance.quantize(2)?
                    ),
                );
                (Some(d("0")), "NONE")
            }
        },
        Some(balance) => {
            let deficit = d("0").subtract(balance);
            let penalty = deficit
                .multiply(&d("1000000"))
                .divide(&intensity.as_ref().unwrap().multiply(&d("41000")))?
                .multiply(&d("2400"));
            let pool_cost = pool.as_ref().map(|pool| deficit.multiply(pool));
            if basis == Some("POOL") {
                if pool_cost.is_none() {
                    unique(&mut missing, "fueleu_pool_price_eur_per_tco2e".into());
                }
                (pool_cost, "POOL")
            } else if basis != Some("PENALTY")
                && pool_cost
                    .as_ref()
                    .is_some_and(|cost| cost.compare(&penalty) == Ordering::Less)
            {
                (pool_cost, "POOL")
            } else {
                (Some(penalty), "PENALTY")
            }
        }
    };
    let year = request
        .assumptions
        .reporting_year
        .unwrap_or(request.start_date.year());
    let ets = if unknown_blend {
        fuel_cost = None;
        None
    } else if co2.compare(&d("0")) == Ordering::Equal {
        Some(d("0"))
    } else if let Some(eua) = &request.assumptions.eua_price_eur_per_tco2 {
        Some(
            co2.multiply(&number(eua)?)
                .multiply(&d(if year == 2025 { "70" } else { "100" }))
                .divide(&d("100"))?,
        )
    } else {
        unique(&mut missing, "eua_price_eur_per_tco2".into());
        None
    };
    let needs_rate = [ets.as_ref(), fueleu_cost.as_ref()]
        .into_iter()
        .flatten()
        .any(|cost| cost.compare(&d("0")) != Ordering::Equal);
    let rate = request
        .assumptions
        .usd_per_eur
        .as_ref()
        .map(number)
        .transpose()?;
    if needs_rate && rate.is_none() {
        unique(&mut missing, "usd_per_eur".into());
    }
    let total = match (&fuel_cost, &ets, &fueleu_cost) {
        (Some(fuel), Some(ets), Some(fueleu)) if !needs_rate || rate.is_some() => {
            Some(fuel.add(&if needs_rate {
                ets.add(fueleu).multiply(rate.as_ref().unwrap())
            } else {
                d("0")
            }))
        }
        _ => None,
    };
    let label = id
        .split('_')
        .map(|word| {
            let lower = word.to_lowercase();
            format!("{}{}", lower[..1].to_uppercase(), &lower[1..])
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(
        json!({"strategy_id":id,"label":label,"completeness":if total.is_some(){"COMPLETE"}else{"PARTIAL"},"fuel_uses":uses,"in_scope_energy_mj":energy.quantize(0)?,"ghg_intensity_gco2e_per_mj":rounded(intensity.as_ref(),4)?,"fueleu_target_gco2e_per_mj":target.quantize(4)?,"fueleu_balance_tco2e":rounded(balance.as_ref(),2)?,"intensity_sources":sources,"fuel_cost_usd":rounded(fuel_cost.as_ref(),2)?,"eu_ets_cost_eur":rounded(ets.as_ref(),2)?,"fueleu_cost_eur":rounded(fueleu_cost.as_ref(),2)?,"fueleu_cost_basis":cost_basis,"total_cost_usd":rounded(total.as_ref(),2)?,"missing_inputs":missing}),
    )
}

pub fn required_price_lookups(input: &Json) -> Result<Json, String> {
    let request = parse_request(input)?;
    validate(&request)?;
    let (_, segments) = legs(&request)?;
    let has_conventional = request
        .vessel
        .compatible_fuels
        .iter()
        .any(|fuel| fuel == conventional());
    let mut lookups = Vec::new();
    for pair in segments.chunks(2) {
        if has_conventional && !pair.iter().any(|segment| positive(&segment.scope)) {
            continue;
        }
        let segment = &pair[0];
        for fuel in &request.vessel.compatible_fuels {
            if fuel == conventional() {
                continue;
            }
            let available = available(fuel, &segment.port);
            let lookup = json!({"fuel":fuel,"port":segment.port,"departure_date":segment.departure.to_string()});
            if available && !lookups.contains(&lookup) {
                lookups.push(lookup);
            }
        }
    }
    Ok(json!(lookups))
}

fn validate(request: &Request) -> Result<(), String> {
    fn bound(value: &Json, low: &str, high: &str, exclusive: bool) -> Result<(), String> {
        let value = number(value)?;
        if value.compare(&d(low)) == Ordering::Less
            || (exclusive && value.compare(&d(low)) == Ordering::Equal)
            || value.compare(&d(high)) == Ordering::Greater
        {
            return Err("value outside supported range".into());
        }
        Ok(())
    }
    if request
        .vessel
        .name
        .as_ref()
        .is_some_and(|name| name.chars().count() > 120)
    {
        return Err("vessel name exceeds 120 characters".into());
    }
    if request
        .vessel
        .imo_number
        .as_ref()
        .is_some_and(|imo| imo.chars().count() != 7 || !imo.chars().all(|c| c.is_numeric()))
    {
        return Err("imo_number must contain seven digits".into());
    }
    bound(&request.vessel.speed_knots, "1", "30", false)?;
    bound(&request.vessel.sea_consumption_mt_per_day, "0", "400", true)?;
    bound(
        &request.vessel.berth_consumption_mt_per_day,
        "0",
        "100",
        false,
    )?;
    let fuels = &request.vessel.compatible_fuels;
    if fuels.is_empty() || fuels.len() > properties()["fuels"].as_object().unwrap().len() {
        return Err("invalid compatible_fuels length".into());
    }
    let mut seen = std::collections::HashSet::new();
    for fuel in fuels {
        if properties()["fuels"].get(fuel).is_none() {
            return Err("unsupported planner fuel".into());
        }
        if !seen.insert(fuel) {
            return Err("compatible_fuels must not repeat a fuel".into());
        }
    }
    if request.legs.is_empty() || request.legs.len() > 10 {
        return Err("voyage must have one to ten legs".into());
    }
    for leg in &request.legs {
        if leg.from_port == leg.to_port {
            return Err("A leg must connect two different ports".into());
        }
        if !["DEFAULT", "CAPE_OF_GOOD_HOPE"].contains(&leg.route.as_str()) {
            return Err("unsupported route".into());
        }
        if let Some(distance) = &leg.distance_nm {
            bound(distance, "0", "30000", true)?;
        }
        bound(&leg.berth_days_at_destination, "0", "60", false)?;
    }
    for pair in request.legs.windows(2) {
        if pair[0].to_port != pair[1].from_port {
            return Err(format!(
                "Legs must connect: a leg ends at {} but the next leg starts at {}",
                pair[0].to_port, pair[1].from_port
            ));
        }
    }
    for price in request.assumptions.vlsfo_price_usd_per_mt.values() {
        bound(price, "0", "10000", true)?;
    }
    for (value, high) in [
        (&request.assumptions.eua_price_eur_per_tco2, "1000"),
        (&request.assumptions.fueleu_pool_price_eur_per_tco2e, "5000"),
        (&request.assumptions.usd_per_eur, "10"),
    ] {
        if let Some(value) = value {
            bound(value, "0", high, true)?;
        }
    }
    for value in request.assumptions.ghg_intensity_gco2e_per_mj.values() {
        bound(value, "0", "200", false)?;
    }
    if request
        .assumptions
        .reporting_year
        .is_some_and(|year| !(2025..=2050).contains(&year))
    {
        return Err("reporting_year outside supported range".into());
    }
    Ok(())
}

pub fn voyage_plan(request: &Json, prices: &Json, today: NaiveDate) -> Result<Json, String> {
    let request = parse_request(request)?;
    if !(-30..=730).contains(&(request.start_date - today).num_days()) {
        return Err("start_date must be within 30 days before and 730 days after today".into());
    }
    validate(&request)?;
    let (legs, segments) = legs(&request)?;
    let year = request
        .assumptions
        .reporting_year
        .unwrap_or(request.start_date.year());
    let target = fueleu::mcp_target(year).map_err(|error| error.to_string())?;
    let mut warnings = Vec::new();
    let mut strategies = Vec::new();
    let has_conventional = request
        .vessel
        .compatible_fuels
        .iter()
        .any(|fuel| fuel == conventional());
    if has_conventional {
        for basis in ["PENALTY", "POOL"] {
            strategies.push(strategy(
                &request,
                prices,
                &segments,
                &target,
                StrategySpec {
                    id: &format!("CONVENTIONAL_{basis}"),
                    green: None,
                    minimum: false,
                    basis: Some(basis),
                },
                &mut warnings,
            )?);
        }
    }
    let in_scope: Vec<_> = segments
        .iter()
        .filter(|segment| positive(&segment.scope))
        .collect();
    for fuel in &request.vessel.compatible_fuels {
        if fuel == conventional() || (has_conventional && in_scope.is_empty()) {
            continue;
        }
        unique(
            &mut warnings,
            "Green fuels need a proof of sustainability for zero ETS CO2 and FuelEU credit.".into(),
        );
        let mut minimum = has_conventional;
        for segment in &in_scope {
            let conventional = choice(&request, prices, segment, conventional())?
                .intensity
                .ok_or("missing conventional intensity")?;
            let green = choice(&request, prices, segment, fuel)?.intensity;
            if conventional.compare(&target) != Ordering::Greater
                || green.is_some_and(|green| green.compare(&target) != Ordering::Less)
            {
                minimum = false;
                unique(
                    &mut warnings,
                    format!(
                        "{fuel}_MIN_BLEND skipped: VLSFO is already compliant or green intensity is at or above the target."
                    ),
                );
                break;
            }
        }
        if minimum {
            strategies.push(strategy(
                &request,
                prices,
                &segments,
                &target,
                StrategySpec {
                    id: &format!("{fuel}_MIN_BLEND"),
                    green: Some(fuel),
                    minimum: true,
                    basis: None,
                },
                &mut warnings,
            )?);
        }
        strategies.push(strategy(
            &request,
            prices,
            &segments,
            &target,
            StrategySpec {
                id: &format!("{fuel}_FULL"),
                green: Some(fuel),
                minimum: false,
                basis: None,
            },
            &mut warnings,
        )?);
    }
    let mut best: Option<&Json> = None;
    for strategy in &strategies {
        if strategy["completeness"] == "COMPLETE"
            && best.is_none_or(|best| {
                number(&strategy["total_cost_usd"])
                    .unwrap()
                    .compare(&number(&best["total_cost_usd"]).unwrap())
                    == Ordering::Less
            })
        {
            best = Some(strategy);
        }
    }
    let (recommended, reason) = if let Some(best) = best {
        (
            best["strategy_id"].clone(),
            format!(
                "{} has the lowest complete total: USD {}.",
                best["strategy_id"].as_str().unwrap(),
                best["total_cost_usd"].as_str().unwrap()
            ),
        )
    } else {
        let mut missing = Vec::new();
        for strategy in &strategies {
            for name in strategy["missing_inputs"].as_array().unwrap() {
                unique(&mut missing, name.as_str().unwrap().into());
            }
        }
        (
            Json::Null,
            format!(
                "No complete strategy.{}",
                if missing.is_empty() {
                    String::new()
                } else {
                    format!(" Supply: {}.", missing.join(", "))
                }
            ),
        )
    };
    Ok(
        json!({"disclaimer":"Indicative planning estimate from Verdaxis market data and the caller's assumptions. It is not a quote, an offer, or advice to trade. Only an order on the Verdaxis orderbook is executable.","reporting_year":year,"legs":legs,"strategies":strategies,"recommended_strategy_id":recommended,"recommendation_reason":reason,"assumptions":["Fuel for a leg and its destination berth stay is bought at the departure port on the departure date.","Consumption uses a 41,000 MJ/t VLSFO-equivalent energy basis.","FuelEU is an annual per-ship balance; this is the voyage contribution, not the annual penalty after banking, borrowing and pooling.","EU scope ignores outermost-region and transshipment rules.","ETS methane and N2O are not included."],"warnings":warnings}),
    )
}
