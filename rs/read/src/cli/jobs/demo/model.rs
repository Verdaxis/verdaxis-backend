//! The disclosed demo book, as pure data: prices, certification schemes,
//! tradable windows, expiries and the coverage orders (ports of
//! `app/seeds/market_seed.py`, `app/services/availability_windows.py` and
//! `build_demo_market_coverage` in `app/services/demo_activity.py`).
//!
//! Every value here must equal Python's for the same reference instant: the
//! coverage refresh compares these fields with the stored rows, and a
//! difference would re-queue the whole demo book on the first Rust run. The
//! parity test runs the Python builder and compares every row.
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use sha1::{Digest, Sha1};
use uuid::Uuid;

pub const SPOT_WINDOW: &str = "SPOT";
/// Rolling five-year delivery horizon.
pub const FORWARD_QUARTER_COUNT: i32 = 5 * 4;
pub const DEMO_COVERAGE_OPERATION: &str = "DEMO_COVERAGE";
pub const DEMO_COVERAGE_LEVELS_PER_SIDE: usize = 10;
pub const CERTIFICATION_SCHEMES: [&str; 3] = ["ISCC EU", "ISCC PLUS", "REDcert EU"];
pub const B30_SPECIFICATION_STANDARD: &str = "ISO 8217:2024 RF 380";
pub const B100_SPECIFICATION_STANDARD: &str = "ISO 8217:2024 DFA";
/// Synthetic forward carry only; not an assessed forward-price forecast.
const DEMO_ANNUAL_CONTANGO_RATE: Decimal = Decimal::from_parts(3, 0, 0, false, 2);
const SLICE_SCHEME_NAMESPACE: Uuid = Uuid::from_u128(0xc4d5e6f7_a8b9_4012_9abc_def123456789);

/// The demo book data (`deploy/rust/demo-book.json`): price bands per product
/// and port, CI ranges and energy density, generated from Python's
/// `app/seeds/market_seed.py` (demo scenarios, not Verdaxis assessments). It
/// is data, not code, so no product name is compiled into this source.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoBook {
    pub version: u32,
    pub source: String,
    pub products: Vec<DemoProduct>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoProduct {
    pub name: String,
    pub ci_low: f64,
    pub ci_high: f64,
    pub energy_density_mj_kg: f64,
    pub ports: Vec<DemoPort>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoPort {
    pub name: String,
    pub bid_low: f64,
    pub bid_high: f64,
    pub ask_low: f64,
    pub ask_high: f64,
}

impl DemoPort {
    pub fn band(&self) -> Band {
        (self.bid_low, self.bid_high, self.ask_low, self.ask_high)
    }
}

/// `(bid_lo, bid_hi, ask_lo, ask_hi)` in USD/MT.
pub type Band = (f64, f64, f64, f64);

/// The committed demo book, parsed once.
pub fn demo_book() -> &'static DemoBook {
    static BOOK: std::sync::LazyLock<DemoBook> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../../../../../../deploy/rust/demo-book.json"))
            .expect("the committed demo book parses")
    });
    &BOOK
}

// ---- Python number semantics ---------------------------------------------------

/// `Decimal(str(value))` for a finite float: the shortest round-trip text.
fn decimal_of_float(value: f64) -> Decimal {
    format!("{value}").parse().expect("a finite float")
}

/// Python's `round(value, 2)` for a float: the nearest two-place decimal of
/// the exact binary value, back to a float.
fn python_round2(value: f64) -> f64 {
    format!("{value:.2}").parse().expect("a finite float")
}

/// `quantize(Decimal("0.01"))` with Python's default ROUND_HALF_EVEN.
/// The result has exactly two places, as Python's quantize gives it.
pub fn cents(value: Decimal) -> Decimal {
    let mut rounded = value.round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven);
    rounded.rescale(2);
    rounded
}

/// `_decimal(value)`: `Decimal(str(value)).quantize(Decimal("0.01"))`.
pub fn two_places(value: f64) -> Decimal {
    cents(decimal_of_float(value))
}

// ---- windows ---------------------------------------------------------------------

/// Spot, the remaining months of the current quarter, then the next
/// [`FORWARD_QUARTER_COUNT`] quarters.
pub fn tradable_windows(today: NaiveDate) -> Vec<String> {
    let quarter = (today.month() as i32 - 1) / 3 + 1;
    let mut windows = vec![SPOT_WINDOW.to_owned()];
    for month in today.month() as i32..=quarter * 3 {
        windows.push(format!("{}-{month:02}", today.year()));
    }
    for index in 0..FORWARD_QUARTER_COUNT {
        let absolute = today.year() * 4 + (quarter - 1) + 1 + index;
        windows.push(format!("{}-Q{}", absolute / 4, absolute % 4 + 1));
    }
    windows
}

fn midnight(date: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_time(NaiveTime::MIN))
}

fn first_after_month(year: i32, month: u32) -> NaiveDate {
    if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .expect("a valid month")
}

/// `(year, month)` of a month window, `(year, quarter)` of a quarter window.
enum Window {
    Spot,
    Month(i32, u32),
    Quarter(i32, u32),
}

fn parse_window(window: &str) -> Window {
    if window == SPOT_WINDOW {
        return Window::Spot;
    }
    let (year, rest) = window.split_at(4);
    let year: i32 = year.parse().expect("a canonical window year");
    match rest.strip_prefix("-Q") {
        Some(quarter) => Window::Quarter(year, quarter.parse().expect("a canonical quarter")),
        None => Window::Month(year, rest[1..].parse().expect("a canonical month")),
    }
}

/// `availability_window_expiry`: Spot 24 h after `observed`; a month or a
/// quarter at midnight UTC after its last day.
pub fn window_expiry(window: &str, observed: DateTime<Utc>) -> DateTime<Utc> {
    match parse_window(window) {
        Window::Spot => observed + Duration::hours(24),
        Window::Month(year, month) => midnight(first_after_month(year, month)),
        Window::Quarter(year, quarter) => midnight(first_after_month(year, quarter * 3)),
    }
}

fn window_start(window: &str, today: NaiveDate) -> NaiveDate {
    match parse_window(window) {
        Window::Spot => today,
        Window::Month(year, month) => {
            NaiveDate::from_ymd_opt(year, month, 1).expect("a valid month")
        }
        Window::Quarter(year, quarter) => {
            NaiveDate::from_ymd_opt(year, (quarter - 1) * 3 + 1, 1).expect("a valid quarter")
        }
    }
}

// ---- prices and schemes ----------------------------------------------------------

/// `_slice_certification_scheme`: uuid5 of the slice, modulo the schemes.
pub fn slice_certification_scheme(product: &str, port: &str, window: &str) -> &'static str {
    let mut hasher = Sha1::new();
    hasher.update(SLICE_SCHEME_NAMESPACE.as_bytes());
    hasher.update(format!("{product}|{port}|{window}").as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let index = u128::from_be_bytes(bytes) % CERTIFICATION_SCHEMES.len() as u128;
    CERTIFICATION_SCHEMES[index as usize]
}

/// Python's `timedelta / 2`: microseconds, halves rounded to even.
fn half_duration_micros(micros: i64) -> i64 {
    let half = micros.div_euclid(2);
    if micros.rem_euclid(2) == 1 && half % 2 != 0 {
        half + 1
    } else {
        half
    }
}

/// `_window_premium`: ACT/365 contango to the middle of the remaining
/// delivery period; zero for Spot and for a period already over.
fn window_premium(window: &str, spot_midpoint: Decimal, reference_date: NaiveDate) -> Decimal {
    if window == SPOT_WINDOW {
        return Decimal::ZERO;
    }
    let valuation = midnight(reference_date);
    let start = midnight(window_start(window, reference_date));
    let end = window_expiry(window, valuation);
    let remaining_start = start.max(valuation);
    if remaining_start >= end {
        return Decimal::ZERO;
    }
    let span = (end - remaining_start)
        .num_microseconds()
        .expect("a bounded span");
    let delivery_micros = (remaining_start - valuation)
        .num_microseconds()
        .expect("a bounded span")
        + half_duration_micros(span);
    // `total_seconds()` is microseconds / 10**6 as a correctly rounded float.
    let seconds = delivery_micros as f64 / 1e6;
    let years = decimal_of_float(seconds) / Decimal::from(365 * 86_400);
    spot_midpoint * DEMO_ANNUAL_CONTANGO_RATE * years
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Bid,
    Ask,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bid => "BID",
            Self::Ask => "ASK",
        }
    }
}

/// `_seed_price_for_slice`.
pub fn seed_price(
    side: Side,
    band: Band,
    window: &str,
    depth_index: usize,
    reference_date: NaiveDate,
) -> Decimal {
    let (bid_lo, bid_hi, ask_lo, ask_hi) = band;
    let best_bid = decimal_of_float(python_round2((bid_lo + bid_hi) / 2.0));
    let natural_ask = decimal_of_float(python_round2((ask_lo + ask_hi) / 2.0));
    let spread_floor = decimal_of_float(python_round2((ask_lo - bid_hi) * 0.6).max(12.0));
    let ladder_step = decimal_of_float(python_round2((ask_hi - ask_lo) * 0.18).max(4.0));
    let best_ask = natural_ask.max(best_bid + spread_floor);
    let premium = window_premium(window, (best_bid + best_ask) / Decimal::TWO, reference_date);
    let depth = Decimal::from(depth_index);
    cents(match side {
        Side::Bid => best_bid + premium - ladder_step * depth,
        Side::Ask => best_ask + premium + ladder_step * depth,
    })
}

/// The B30 and B100 ask disclosure (`ask_seed_metadata` with a scheme).
pub struct BlendDisclosure {
    pub specification_standard: &'static str,
    pub feedstock: &'static str,
    pub carbon_intensity_method: &'static str,
}

pub fn blend_disclosure(product: &str) -> Option<BlendDisclosure> {
    match product {
        "B30" => Some(BlendDisclosure {
            specification_standard: B30_SPECIFICATION_STANDARD,
            feedstock: "Indicative demo: waste-derived FAME (30% by volume) + VLSFO (70% by volume)",
            carbon_intensity_method: "Indicative demo whole-blend CI and LCV; includes fossil VLSFO; not certified batch values",
        }),
        "B100" => Some(BlendDisclosure {
            specification_standard: B100_SPECIFICATION_STANDARD,
            feedstock: "Indicative demo: 100% waste-derived FAME; excludes HVO",
            carbon_intensity_method: "Indicative demo whole-fuel FAME CI and LCV; not certified batch values",
        }),
        _ => None,
    }
}

// ---- coverage --------------------------------------------------------------------

/// One coverage order: the managed fields and the identity of its slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageOrder {
    pub key: String,
    pub request_hash: String,
    pub organization_id: Uuid,
    pub side: Side,
    pub product: &'static str,
    pub port: &'static str,
    pub product_id: Uuid,
    pub delivery_point_id: Uuid,
    pub window: String,
    pub quantity_mt: Decimal,
    pub price_per_mt_usd: Decimal,
    pub certifications: Vec<String>,
    pub certification_declared: bool,
    pub certification_scheme: &'static str,
    pub specification_standard: Option<&'static str>,
    pub msds_available: bool,
    pub is_verdaxis_verified: bool,
    pub carbon_intensity_gco2_mj: Option<Decimal>,
    pub carbon_intensity_method: Option<&'static str>,
    pub energy_density_mj_kg: Option<Decimal>,
    pub feedstock: Option<&'static str>,
    pub origin: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// Where the catalog names resolve to identifiers.
pub trait Catalog {
    fn product_id(&self, name: &str) -> Uuid;
    fn delivery_point_id(&self, name: &str) -> Uuid;
}

pub fn sha256_hex(text: &str) -> String {
    use sha2::Sha256;
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `build_demo_market_coverage(now)`: the rolling disclosed baseline.
pub fn build_coverage(
    now: DateTime<Utc>,
    catalog: &dyn Catalog,
    buyers: &[Uuid],
    suppliers: &[Uuid],
) -> Vec<CoverageOrder> {
    let today = now.date_naive();
    let windows = tradable_windows(today);
    let mut orders = Vec::new();
    for book_product in &demo_book().products {
        let product = book_product.name.as_str();
        let (ci_lo, ci_hi, energy_density) = (
            book_product.ci_low,
            book_product.ci_high,
            book_product.energy_density_mj_kg,
        );
        for book_port in &book_product.ports {
            let (port, band) = (book_port.name.as_str(), book_port.band());
            for window in &windows {
                let scheme = slice_certification_scheme(product, port, window);
                for side in [Side::Bid, Side::Ask] {
                    for depth in 0..DEMO_COVERAGE_LEVELS_PER_SIDE {
                        let ordinal = orders.len();
                        let quantity = if depth == 0 {
                            Decimal::from(1500)
                        } else {
                            Decimal::from(1000)
                        };
                        let key = format!("{product}|{port}|{window}|{}|{depth}", side.as_str());
                        let mut order = CoverageOrder {
                            request_hash: sha256_hex(&key),
                            key,
                            organization_id: match side {
                                Side::Bid => buyers[ordinal % buyers.len()],
                                Side::Ask => suppliers[ordinal % suppliers.len()],
                            },
                            side,
                            product,
                            port,
                            product_id: catalog.product_id(product),
                            delivery_point_id: catalog.delivery_point_id(port),
                            window: window.clone(),
                            quantity_mt: quantity,
                            price_per_mt_usd: seed_price(side, band, window, depth, today),
                            certifications: Vec::new(),
                            certification_declared: false,
                            certification_scheme: scheme,
                            specification_standard: None,
                            msds_available: false,
                            is_verdaxis_verified: false,
                            carbon_intensity_gco2_mj: None,
                            carbon_intensity_method: None,
                            energy_density_mj_kg: None,
                            feedstock: None,
                            origin: None,
                            expires_at: window_expiry(window, now),
                            created_at: now - Duration::minutes((ordinal % 360) as i64),
                        };
                        if side == Side::Ask {
                            order.certifications = vec![scheme.to_owned()];
                            order.certification_declared = true;
                            order.specification_standard = Some("Supplier specification");
                            order.msds_available = true;
                            order.is_verdaxis_verified = true;
                            order.carbon_intensity_gco2_mj =
                                Some(two_places((ci_lo + ci_hi) / 2.0));
                            order.carbon_intensity_method = Some("Indicative demo pathway");
                            order.energy_density_mj_kg = Some(two_places(energy_density));
                            order.feedstock = Some("Indicative demo feedstock");
                            order.origin = Some(format!("{port} demo terminal"));
                            if let Some(blend) = blend_disclosure(product) {
                                order.specification_standard = Some(blend.specification_standard);
                                order.feedstock = Some(blend.feedstock);
                                order.carbon_intensity_method = Some(blend.carbon_intensity_method);
                                order.is_verdaxis_verified = false;
                            }
                        }
                        orders.push(order);
                    }
                }
            }
        }
    }
    orders
}

/// `DEMO_SEED_BUYERS` (app/demo_identities.py), in order.
pub const DEMO_SEED_BUYERS: [Uuid; 5] = [
    Uuid::from_u128(0x4da7b285_34ee_5443_9406_f96b4ed1a251),
    Uuid::from_u128(0x0dbce576_2026_5925_ab66_674d505e98ad),
    Uuid::from_u128(0x3cd0bc8a_0e92_5eb8_9ae3_3c87a7460a6a),
    Uuid::from_u128(0x277491df_cb0d_5f2d_a2cf_5746829c6da6),
    Uuid::from_u128(0x3b302066_d65c_5c3e_8fcc_70b3da3bcafd),
];

/// `DEMO_SEED_SUPPLIERS` (app/demo_identities.py), in order.
pub const DEMO_SEED_SUPPLIERS: [Uuid; 5] = [
    Uuid::from_u128(0x79609f48_0a3e_560e_a1e1_63d90601d84a),
    Uuid::from_u128(0x2c4e387e_de22_5adb_ad88_9274ba84ebe1),
    Uuid::from_u128(0x612953c7_567a_58b3_bc42_ee817d2bbe74),
    Uuid::from_u128(0x93ccda09_54b3_53ee_afc0_759d3048161f),
    Uuid::from_u128(0x82426590_0963_5486_9b05_f81e97afe6ef),
];

/// The ten demo seed organizations, buyers then suppliers: the only
/// organizations coverage may write.
pub const DEMO_SEED_ORGANIZATIONS: [Uuid; 10] = [
    DEMO_SEED_BUYERS[0],
    DEMO_SEED_BUYERS[1],
    DEMO_SEED_BUYERS[2],
    DEMO_SEED_BUYERS[3],
    DEMO_SEED_BUYERS[4],
    DEMO_SEED_SUPPLIERS[0],
    DEMO_SEED_SUPPLIERS[1],
    DEMO_SEED_SUPPLIERS[2],
    DEMO_SEED_SUPPLIERS[3],
    DEMO_SEED_SUPPLIERS[4],
];

/// The two demo activity organizations (`DEMO_ACTIVITY_*_ORG_ID`).
pub const DEMO_ACTIVITY_BUYER_ORG_ID: Uuid =
    Uuid::from_u128(0x7cc77115_0a9f_4ec4_8c74_05aa10050111);
pub const DEMO_ACTIVITY_SELLER_ORG_ID: Uuid =
    Uuid::from_u128(0xd1e43e55_3fb0_4b5e_9f0b_93aa10050222);

// ---- activity tick ---------------------------------------------------------------

/// Python's tick quantities (`_quantity`), in metric tons.
pub const TICK_QUANTITIES_MT: [i64; 7] = [500, 750, 1000, 1250, 1500, 2000, 2500];
/// The length of one activity tick: one matched pair per tick at most.
pub const TICK_SECONDS: i64 = 300;

/// Python's `_activity_tick`: the start of the five-minute tick holding `now`.
pub fn activity_tick(now: DateTime<Utc>) -> DateTime<Utc> {
    let seconds = now.timestamp().div_euclid(TICK_SECONDS) * TICK_SECONDS;
    Utc.timestamp_opt(seconds, 0).single().unwrap_or(now)
}

/// The tick's draws. Python seeds its Mersenne Twister with the tick key; its
/// draws are not reproduced (only one owner runs per environment, and the
/// tick-trade check makes a repeat a no-op). This splitmix64 sequence is
/// seeded by the same key, so a rerun of one tick draws the same pair.
struct TickDraws(u64);

impl TickDraws {
    fn new(tick: DateTime<Utc>) -> Self {
        Self(tick.timestamp().div_euclid(TICK_SECONDS) as u64)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn index(&mut self, length: usize) -> usize {
        (self.next() % length as u64) as usize
    }
    /// A value in `[low, high]`, like Python's `uniform`.
    fn uniform(&mut self, low: f64, high: f64) -> f64 {
        let unit = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        low + (high - low) * unit
    }
}

/// The disclosure of the tick's ask (Python's `_ask_metadata`).
#[derive(Debug, Clone, PartialEq)]
pub struct TickAsk {
    pub certification_scheme: &'static str,
    pub specification_standard: &'static str,
    pub is_verdaxis_verified: bool,
    pub carbon_intensity_gco2_mj: Decimal,
    pub carbon_intensity_method: &'static str,
    pub energy_density_mj_kg: Decimal,
    pub feedstock: &'static str,
    pub origin: String,
}

/// One tick's matched pair: the slice, the quantity, the midpoint price and
/// the ask's disclosure.
#[derive(Debug, Clone, PartialEq)]
pub struct TickChoice {
    pub tick: DateTime<Utc>,
    pub product: &'static str,
    pub port: &'static str,
    pub window: String,
    pub quantity_mt: Decimal,
    pub price_per_mt_usd: Decimal,
    pub expires_at: DateTime<Utc>,
    pub ask: TickAsk,
}

/// Python's `_activity_slice`, `_quantity`, `_price` and `_ask_metadata` for
/// the tick holding `now`.
pub fn tick_choice(now: DateTime<Utc>) -> TickChoice {
    let tick = activity_tick(now);
    let today = tick.date_naive();
    let mut draws = TickDraws::new(tick);
    let book = demo_book();
    let product = &book.products[draws.index(book.products.len())];
    let port = &product.ports[draws.index(product.ports.len())];
    let windows = tradable_windows(today);
    let window = windows[draws.index(windows.len())].clone();
    let quantity = Decimal::from(TICK_QUANTITIES_MT[draws.index(TICK_QUANTITIES_MT.len())]);
    let band = port.band();
    let bid = seed_price(Side::Bid, band, &window, 0, today);
    let ask = seed_price(Side::Ask, band, &window, 0, today);
    let scheme = slice_certification_scheme(&product.name, &port.name, &window);
    let mut disclosure = TickAsk {
        certification_scheme: scheme,
        specification_standard: "ISO 8217 / supplier specification",
        is_verdaxis_verified: true,
        carbon_intensity_gco2_mj: two_places(python_round2(
            draws.uniform(product.ci_low, product.ci_high),
        )),
        carbon_intensity_method: "ISCC EU default pathway",
        energy_density_mj_kg: two_places(product.energy_density_mj_kg),
        feedstock: "Demo feedstock disclosure",
        origin: format!("{} demo terminal", port.name),
    };
    if let Some(blend) = blend_disclosure(&product.name) {
        disclosure.specification_standard = blend.specification_standard;
        disclosure.feedstock = blend.feedstock;
        disclosure.carbon_intensity_method = blend.carbon_intensity_method;
        disclosure.is_verdaxis_verified = false;
    }
    TickChoice {
        tick,
        product: &product.name,
        port: &port.name,
        expires_at: window_expiry(&window, tick),
        window,
        quantity_mt: quantity,
        price_per_mt_usd: cents((bid + ask) / Decimal::TWO),
        ask: disclosure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn the_tick_is_the_five_minute_start_and_its_draw_is_stable() {
        let start = at("2026-11-04T06:15:00Z");
        assert_eq!(activity_tick(at("2026-11-04T06:15:00Z")), start);
        assert_eq!(activity_tick(at("2026-11-04T06:19:59.999Z")), start);
        assert_eq!(
            activity_tick(at("2026-11-04T06:20:00Z")),
            at("2026-11-04T06:20:00Z")
        );
        let first = tick_choice(at("2026-11-04T06:15:01Z"));
        assert_eq!(
            first,
            tick_choice(at("2026-11-04T06:19:59Z")),
            "one tick, one draw"
        );
        assert_eq!(first.tick, start);
        assert!(TICK_QUANTITIES_MT.contains(&first.quantity_mt.try_into().unwrap()));
        assert_eq!(first.price_per_mt_usd.scale(), 2);
        assert_eq!(first.expires_at, window_expiry(&first.window, start));
        let product = demo_book()
            .products
            .iter()
            .find(|product| product.name == first.product)
            .unwrap();
        assert!(product.ports.iter().any(|port| port.name == first.port));
        let ci = first.ask.carbon_intensity_gco2_mj;
        assert!(ci >= two_places(product.ci_low) && ci <= two_places(product.ci_high));
    }

    #[test]
    fn the_tick_price_is_the_book_midpoint() {
        let choice = tick_choice(at("2026-11-04T06:15:00Z"));
        let book = demo_book();
        let port = book
            .products
            .iter()
            .find(|product| product.name == choice.product)
            .and_then(|product| product.ports.iter().find(|port| port.name == choice.port))
            .unwrap();
        let today = choice.tick.date_naive();
        let bid = seed_price(Side::Bid, port.band(), &choice.window, 0, today);
        let ask = seed_price(Side::Ask, port.band(), &choice.window, 0, today);
        assert!(bid <= choice.price_per_mt_usd && choice.price_per_mt_usd <= ask);
        assert_eq!(choice.price_per_mt_usd, cents((bid + ask) / Decimal::TWO));
    }
}
