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

pub const PORTS: [&str; 8] = [
    "Dalian",
    "Busan",
    "Shanghai",
    "Singapore",
    "Rotterdam",
    "Houston",
    "Los Angeles",
    "Santos",
];

/// Demo price bands in USD/MT: product -> one `(bid_lo, bid_hi, ask_lo,
/// ask_hi)` per port in [`PORTS`] order. Python's dict order is kept. These
/// are demo scenarios, not Verdaxis assessments (sources in market_seed.py).
/// `(bid_lo, bid_hi, ask_lo, ask_hi)` in USD/MT.
pub type Band = (f64, f64, f64, f64);

pub const PRICING: [(&str, [Band; 8]); 6] = [
    (
        "Bio Methanol",
        [
            (890.0, 930.0, 950.0, 990.0),
            (910.0, 950.0, 970.0, 1010.0),
            (885.0, 925.0, 945.0, 985.0),
            (930.0, 975.0, 995.0, 1040.0),
            (900.0, 950.0, 975.0, 1025.0),
            (860.0, 910.0, 935.0, 985.0),
            (900.0, 950.0, 975.0, 1025.0),
            (820.0, 870.0, 895.0, 945.0),
        ],
    ),
    (
        "e-Methanol",
        [
            (1040.0, 1110.0, 1140.0, 1210.0),
            (1070.0, 1140.0, 1170.0, 1240.0),
            (1010.0, 1080.0, 1110.0, 1180.0),
            (1090.0, 1170.0, 1205.0, 1285.0),
            (1190.0, 1280.0, 1320.0, 1410.0),
            (1040.0, 1120.0, 1155.0, 1235.0),
            (1110.0, 1190.0, 1225.0, 1305.0),
            (990.0, 1070.0, 1105.0, 1185.0),
        ],
    ),
    (
        "Bio Ethanol",
        [
            (800.0, 840.0, 855.0, 895.0),
            (810.0, 850.0, 865.0, 905.0),
            (795.0, 835.0, 850.0, 890.0),
            (815.0, 855.0, 870.0, 910.0),
            (780.0, 820.0, 835.0, 875.0),
            (680.0, 710.0, 725.0, 755.0),
            (740.0, 780.0, 795.0, 835.0),
            (730.0, 760.0, 775.0, 805.0),
        ],
    ),
    (
        "Synthetic Ethanol",
        [
            (1140.0, 1210.0, 1245.0, 1315.0),
            (1170.0, 1240.0, 1275.0, 1345.0),
            (1120.0, 1190.0, 1225.0, 1295.0),
            (1190.0, 1260.0, 1295.0, 1365.0),
            (1240.0, 1320.0, 1360.0, 1440.0),
            (1070.0, 1140.0, 1175.0, 1245.0),
            (1130.0, 1200.0, 1235.0, 1305.0),
            (1090.0, 1160.0, 1195.0, 1265.0),
        ],
    ),
    (
        "B30",
        [
            (1085.0, 1105.0, 1105.0, 1125.0),
            (1085.0, 1105.0, 1105.0, 1125.0),
            (1085.0, 1105.0, 1105.0, 1125.0),
            (1085.0, 1105.0, 1105.0, 1125.0),
            (849.0, 869.0, 869.0, 889.0),
            (1085.0, 1105.0, 1105.0, 1125.0),
            (1085.0, 1105.0, 1105.0, 1125.0),
            (1085.0, 1105.0, 1105.0, 1125.0),
        ],
    ),
    ("B100", [(1342.0, 1362.0, 1362.0, 1382.0); 8]),
];

/// Carbon intensity range and energy density: `(ci_lo, ci_hi, MJ/kg)`.
pub fn ci_data(product: &str) -> (f64, f64, f64) {
    match product {
        "Bio Ethanol" => (8.0, 18.0, 26.8),
        "Bio Methanol" => (25.0, 55.0, 19.9),
        "e-Methanol" => (5.0, 20.0, 19.9),
        "Synthetic Ethanol" => (15.0, 35.0, 26.8),
        "B30" => (70.0, 80.0, 39.5),
        "B100" => (15.0, 30.0, 37.2),
        _ => unreachable!("every demo product has CI data"),
    }
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
    for (product, bands) in PRICING {
        let (ci_lo, ci_hi, energy_density) = ci_data(product);
        for (port, band) in PORTS.iter().copied().zip(bands) {
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

/// The two demo activity organizations (`DEMO_ACTIVITY_*_ORG_ID`).
pub const DEMO_ACTIVITY_BUYER_ORG_ID: Uuid =
    Uuid::from_u128(0x7cc77115_0a9f_4ec4_8c74_05aa10050111);
pub const DEMO_ACTIVITY_SELLER_ORG_ID: Uuid =
    Uuid::from_u128(0xd1e43e55_3fb0_4b5e_9f0b_93aa10050222);
