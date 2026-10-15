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
    if value == SPOT_WINDOW {
        return Some(Window::Spot);
    }
    let (year_text, suffix) = value.split_once('-')?;
    if year_text.chars().count() != 4 { return None; }
    let year = year_text.chars().try_fold(0i32, |year, c| {
        python_decimal_digit(c).map(|digit| year * 10 + digit as i32)
    })?;
    match suffix.as_bytes() {
        [b'0'..=b'9', b'0'..=b'9'] => {
            let month: u32 = suffix.parse().ok()?;
            (1..=12).contains(&month).then_some(Window::Month(year, month))
        }
        [b'Q', quarter @ b'1'..=b'4'] => Some(Window::Quarter(year, (quarter - b'0') as u32)),
        [b'C', b'A', b'L'] => Some(Window::Calendar(year)),
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

fn start_date(window: Window, today: NaiveDate) -> Result<NaiveDate, String> {
    let (year, month) = match window {
        Window::Spot => return Ok(today),
        Window::Month(year, month) => (year, month),
        Window::Quarter(year, quarter) => (year, (quarter - 1) * 3 + 1),
        Window::Calendar(year) => (year, 1),
    };
    if year == 0 { return Err("year 0 is out of range".to_owned()); }
    NaiveDate::from_ymd_opt(year, month, 1).ok_or_else(|| "invalid window date".to_owned())
}

/// First calendar day the window covers. Spot resolves to `today`.
pub fn window_start_date(value: &str, today: NaiveDate) -> Result<NaiveDate, String> {
    let canonical = normalize_window(value, today)?;
    let window = parse_canonical(&canonical).ok_or_else(|| BAD_WINDOW.to_owned())?;
    start_date(window, today)
}

/// Canonical ordering key: spot first, then month, then quarter, then
/// calendar year, each within its own start date.
pub fn window_sort_key(value: &str, today: NaiveDate) -> Result<(u8, NaiveDate, u8), String> {
    let canonical = normalize_window(value, today)?;
    let window = parse_canonical(&canonical).ok_or_else(|| BAD_WINDOW.to_owned())?;
    Ok(match window {
        Window::Spot => (0, today, 0),
        Window::Month(..) => (1, start_date(window, today)?, 0),
        Window::Quarter(..) => (1, start_date(window, today)?, 1),
        Window::Calendar(..) => (1, start_date(window, today)?, 2),
    })
}

/// Human label for the monitoring views.
pub fn window_display_label(value: &str) -> Result<String, String> {
    let canonical = normalize_window(value, NaiveDate::MIN)?;
    let window = parse_canonical(&canonical).ok_or_else(|| BAD_WINDOW.to_owned())?;
    Ok(match window {
        Window::Spot => "Spot".to_owned(),
        Window::Month(year, month) => format!("{} {}", MONTH_NAMES[(month - 1) as usize], year),
        Window::Quarter(_, quarter) => format!("Q{quarter} {}", canonical.split_once('-').unwrap().0),
        Window::Calendar(_) => format!("CAL {}", canonical.split_once('-').unwrap().0),
    })
}

// Unicode 15.0 decimal ranges used by the pinned Python 3.12 oracle's \d.
fn python_decimal_digit(c: char) -> Option<u32> {
    const ZEROES: &[u32] = &[
        0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6,
        0xc66, 0xce6, 0xd66, 0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090,
        0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90, 0x1b50, 0x1bb0,
        0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50,
        0xabf0, 0xff10, 0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136,
        0x111d0, 0x112f0, 0x11450, 0x114d0, 0x11650, 0x116c0, 0x11730,
        0x118e0, 0x11950, 0x11c50, 0x11d50, 0x11da0, 0x11f50, 0x16a60,
        0x16ac0, 0x16b50, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6,
        0x1e140, 0x1e2f0, 0x1e4f0, 0x1e950, 0x1fbf0,
    ];
    ZEROES.iter().find_map(|zero| (c as u32).checked_sub(*zero).filter(|n| *n < 10))
}
