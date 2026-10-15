//! Canonical availability-window grammar, aliases, and sort keys.
//!
//! Port of `app/services/availability_windows.py` for the parts shared market
//! reads need: the v1 grammar (`SPOT`, `YYYY-MM`, `YYYY-QN`, `YYYY-CAL`), the
//! legacy alias table, ordering, and display labels. Route-specific rejection
//! codes and tradability stay with the routes that own them.

use chrono::NaiveDate;

pub const SPOT_WINDOW: &str = "SPOT";

/// Legacy display aliases accepted on input, mapped to their canonical code.
/// This is a wire-compatibility table, not product identity.
const LEGACY_WINDOW_ALIASES: [(&str, &str); 22] = [
    ("SPOT", "SPOT"),
    ("Spot", "SPOT"),
    ("Q1_2025", "2025-Q1"),
    ("Q2_2025", "2025-Q2"),
    ("Q3_2025", "2025-Q3"),
    ("Q4_2025", "2025-Q4"),
    ("Q1_2026", "2026-Q1"),
    ("Q2_2026", "2026-Q2"),
    ("Q3_2026", "2026-Q3"),
    ("Q4_2026", "2026-Q4"),
    ("Q1 2025", "2025-Q1"),
    ("Q2 2025", "2025-Q2"),
    ("Q3 2025", "2025-Q3"),
    ("Q4 2025", "2025-Q4"),
    ("Q1 2026", "2026-Q1"),
    ("Q2 2026", "2026-Q2"),
    ("Q3 2026", "2026-Q3"),
    ("Q4 2026", "2026-Q4"),
    ("FORWARD_2027", "2027-CAL"),
    ("FORWARD_2028", "2028-CAL"),
    ("Forward 2027", "2027-CAL"),
    ("Forward 2028", "2028-CAL"),
];

const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const BLANK_WINDOW: &str = "availability_window cannot be blank";
const BAD_WINDOW: &str = "availability_window must be SPOT, YYYY-MM, YYYY-QN, or YYYY-CAL";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Window {
    Spot,
    Month(i32, u32),
    Quarter(i32, u32),
    Calendar(i32),
}

/// Parse the canonical v1 grammar only. Aliases and case folding are handled
/// by [`normalize_window`], not here.
fn parse_canonical(value: &str) -> Option<Window> {
    let bytes = value.as_bytes();
    if value == SPOT_WINDOW {
        return Some(Window::Spot);
    }
    if bytes.len() != 7 && bytes.len() != 8 {
        return None;
    }
    let year: i32 = value.get(0..4)?.parse().ok()?;
    if !value.get(0..4)?.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    match bytes {
        [_, _, _, _, b'-', b'0'..=b'9', b'0'..=b'9'] => {
            let month: u32 = value.get(5..7)?.parse().ok()?;
            (1..=12)
                .contains(&month)
                .then_some(Window::Month(year, month))
        }
        [_, _, _, _, b'-', b'Q', b'1'..=b'4'] => {
            let quarter = (bytes[6] - b'0') as u32;
            Some(Window::Quarter(year, quarter))
        }
        [_, _, _, _, b'-', b'C', b'A', b'L'] => Some(Window::Calendar(year)),
        _ => None,
    }
}

/// Normalize one availability window to its canonical code.
///
/// `today` is part of the shared entry point so callers pass one clock; the
/// v1 grammar itself does not depend on it.
pub fn normalize_window(value: &str, _today: NaiveDate) -> Result<String, String> {
    let raw = value.trim();
    if raw.is_empty() {
        return Err(BLANK_WINDOW.to_owned());
    }
    if let Some((_, canonical)) = LEGACY_WINDOW_ALIASES.iter().find(|(key, _)| *key == raw) {
        return Ok((*canonical).to_owned());
    }
    if raw.to_ascii_uppercase() == SPOT_WINDOW {
        return Ok(SPOT_WINDOW.to_owned());
    }
    if parse_canonical(raw).is_some() {
        return Ok(raw.to_owned());
    }
    Err(BAD_WINDOW.to_owned())
}

/// Whether a value is already in the strict canonical grammar (no aliases,
/// no case folding). This is the form used by stored columns and trade
/// snapshots.
pub fn is_canonical_window(value: &str) -> bool {
    parse_canonical(value).is_some()
}

fn start_date(window: Window, today: NaiveDate) -> NaiveDate {
    match window {
        Window::Spot => today,
        Window::Month(year, month) => NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(today),
        Window::Quarter(year, quarter) => {
            NaiveDate::from_ymd_opt(year, (quarter - 1) * 3 + 1, 1).unwrap_or(today)
        }
        Window::Calendar(year) => NaiveDate::from_ymd_opt(year, 1, 1).unwrap_or(today),
    }
}

/// First calendar day the window covers. Spot resolves to `today`.
pub fn window_start_date(value: &str, today: NaiveDate) -> Result<NaiveDate, String> {
    let canonical = normalize_window(value, today)?;
    let window = parse_canonical(&canonical).ok_or_else(|| BAD_WINDOW.to_owned())?;
    Ok(start_date(window, today))
}

/// Canonical ordering key: spot first, then month, then quarter, then
/// calendar year, each within its own start date.
pub fn window_sort_key(value: &str, today: NaiveDate) -> Result<(u8, NaiveDate, u8), String> {
    let canonical = normalize_window(value, today)?;
    let window = parse_canonical(&canonical).ok_or_else(|| BAD_WINDOW.to_owned())?;
    Ok(match window {
        Window::Spot => (0, today, 0),
        Window::Month(..) => (1, start_date(window, today), 0),
        Window::Quarter(..) => (1, start_date(window, today), 1),
        Window::Calendar(..) => (1, start_date(window, today), 2),
    })
}

/// Human label for the monitoring views.
pub fn window_display_label(value: &str) -> Result<String, String> {
    let canonical = normalize_window(value, NaiveDate::MIN)?;
    let window = parse_canonical(&canonical).ok_or_else(|| BAD_WINDOW.to_owned())?;
    Ok(match window {
        Window::Spot => "Spot".to_owned(),
        Window::Month(year, month) => format!("{} {}", MONTH_NAMES[(month - 1) as usize], year),
        Window::Quarter(year, quarter) => format!("Q{quarter} {year}"),
        Window::Calendar(year) => format!("CAL {year}"),
    })
}
