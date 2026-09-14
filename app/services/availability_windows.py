from __future__ import annotations

import calendar
import re
from datetime import UTC, date, datetime, time, timedelta
from typing import Final


SPOT_WINDOW: Final[str] = "SPOT"
FORWARD_QUARTER_COUNT: Final[int] = 5 * 4  # Rolling five-year delivery horizon.

MONTH_WINDOW_RE: Final[re.Pattern[str]] = re.compile(r"^(?P<year>\d{4})-(?P<month>0[1-9]|1[0-2])$")
QUARTER_WINDOW_RE: Final[re.Pattern[str]] = re.compile(r"^(?P<year>\d{4})-Q(?P<quarter>[1-4])$")
CALENDAR_WINDOW_RE: Final[re.Pattern[str]] = re.compile(r"^(?P<year>\d{4})-CAL$")

LEGACY_WINDOW_ALIASES: Final[dict[str, str]] = {
    "SPOT": SPOT_WINDOW,
    "Spot": SPOT_WINDOW,
    "Q1_2025": "2025-Q1",
    "Q2_2025": "2025-Q2",
    "Q3_2025": "2025-Q3",
    "Q4_2025": "2025-Q4",
    "Q1_2026": "2026-Q1",
    "Q2_2026": "2026-Q2",
    "Q3_2026": "2026-Q3",
    "Q4_2026": "2026-Q4",
    "Q1 2025": "2025-Q1",
    "Q2 2025": "2025-Q2",
    "Q3 2025": "2025-Q3",
    "Q4 2025": "2025-Q4",
    "Q1 2026": "2026-Q1",
    "Q2 2026": "2026-Q2",
    "Q3 2026": "2026-Q3",
    "Q4 2026": "2026-Q4",
    "FORWARD_2027": "2027-CAL",
    "FORWARD_2028": "2028-CAL",
    "Forward 2027": "2027-CAL",
    "Forward 2028": "2028-CAL",
}

MONTH_NAMES: Final[tuple[str, ...]] = (
    "Jan",
    "Feb",
    "Mar",
    "Apr",
    "May",
    "Jun",
    "Jul",
    "Aug",
    "Sep",
    "Oct",
    "Nov",
    "Dec",
)

JSON_SCHEMA_PATTERN: Final[str] = r"^(SPOT|\d{4}-(0[1-9]|1[0-2])|\d{4}-Q[1-4]|\d{4}-CAL)$"


def normalize_availability_window(value: str) -> str:
    raw = value.strip()
    if not raw:
        raise ValueError("availability_window cannot be blank")

    if raw in LEGACY_WINDOW_ALIASES:
        return LEGACY_WINDOW_ALIASES[raw]

    upper = raw.upper()
    if upper == SPOT_WINDOW:
        return SPOT_WINDOW

    if MONTH_WINDOW_RE.fullmatch(raw) or QUARTER_WINDOW_RE.fullmatch(raw) or CALENDAR_WINDOW_RE.fullmatch(raw):
        return raw

    raise ValueError(
        "availability_window must be SPOT, YYYY-MM, YYYY-QN, or YYYY-CAL"
    )


def is_valid_availability_window(value: str) -> bool:
    try:
        normalize_availability_window(value)
    except ValueError:
        return False
    return True


def window_start_date(value: str, *, today: date | None = None) -> date:
    normalized = normalize_availability_window(value)
    if normalized == SPOT_WINDOW:
        return today or date.today()

    month_match = MONTH_WINDOW_RE.fullmatch(normalized)
    if month_match:
        return date(int(month_match.group("year")), int(month_match.group("month")), 1)

    quarter_match = QUARTER_WINDOW_RE.fullmatch(normalized)
    if quarter_match:
        quarter = int(quarter_match.group("quarter"))
        month = ((quarter - 1) * 3) + 1
        return date(int(quarter_match.group("year")), month, 1)

    calendar_match = CALENDAR_WINDOW_RE.fullmatch(normalized)
    if calendar_match:
        return date(int(calendar_match.group("year")), 1, 1)

    raise ValueError(f"Unsupported availability window: {value}")


def availability_window_sort_key(value: str, *, today: date | None = None) -> tuple[int, date, int]:
    normalized = normalize_availability_window(value)
    if normalized == SPOT_WINDOW:
        return (0, today or date.today(), 0)

    if MONTH_WINDOW_RE.fullmatch(normalized):
        return (1, window_start_date(normalized, today=today), 0)

    if QUARTER_WINDOW_RE.fullmatch(normalized):
        return (1, window_start_date(normalized, today=today), 1)

    if CALENDAR_WINDOW_RE.fullmatch(normalized):
        return (1, window_start_date(normalized, today=today), 2)

    return (99, date.max, 99)


def availability_window_display_label(value: str) -> str:
    normalized = normalize_availability_window(value)
    if normalized == SPOT_WINDOW:
        return "Spot"

    month_match = MONTH_WINDOW_RE.fullmatch(normalized)
    if month_match:
        month = int(month_match.group("month"))
        year = int(month_match.group("year"))
        return f"{MONTH_NAMES[month - 1]} {year}"

    quarter_match = QUARTER_WINDOW_RE.fullmatch(normalized)
    if quarter_match:
        return f"Q{quarter_match.group('quarter')} {quarter_match.group('year')}"

    calendar_match = CALENDAR_WINDOW_RE.fullmatch(normalized)
    if calendar_match:
        return f"CAL {calendar_match.group('year')}"

    return normalized


def tradable_availability_windows(
    *,
    today: date | None = None,
    quarter_count: int = FORWARD_QUARTER_COUNT,
) -> list[str]:
    current = today or date.today()
    current_quarter = ((current.month - 1) // 3) + 1
    current_quarter_end_month = current_quarter * 3

    windows = [SPOT_WINDOW]
    for month in range(current.month, current_quarter_end_month + 1):
        windows.append(f"{current.year}-{month:02d}")

    for index in range(quarter_count):
        absolute_quarter = (current.year * 4) + (current_quarter - 1) + 1 + index
        quarter_year = absolute_quarter // 4
        quarter = (absolute_quarter % 4) + 1
        windows.append(f"{quarter_year}-Q{quarter}")

    return windows


def is_tradable_availability_window(
    value: str,
    *,
    today: date | None = None,
    quarter_count: int = FORWARD_QUARTER_COUNT,
) -> bool:
    """Return whether a canonical window is currently open for new orders."""
    normalized = normalize_availability_window(value)
    return normalized in tradable_availability_windows(
        today=today,
        quarter_count=quarter_count,
    )


def availability_window_expiry(
    value: str,
    *,
    observed_at: datetime | None = None,
) -> datetime:
    """Return the exclusive UTC expiry for deterministic synthetic liquidity."""
    observed = observed_at or datetime.now(UTC)
    if observed.tzinfo is None:
        observed = observed.replace(tzinfo=UTC)
    else:
        observed = observed.astimezone(UTC)

    normalized = normalize_availability_window(value)
    if normalized == SPOT_WINDOW:
        return observed + timedelta(hours=24)

    month_match = MONTH_WINDOW_RE.fullmatch(normalized)
    if month_match:
        year = int(month_match.group("year"))
        month = int(month_match.group("month"))
        last_day = calendar.monthrange(year, month)[1]
        return datetime.combine(date(year, month, last_day) + timedelta(days=1), time.min, UTC)

    quarter_match = QUARTER_WINDOW_RE.fullmatch(normalized)
    if quarter_match:
        year = int(quarter_match.group("year"))
        quarter = int(quarter_match.group("quarter"))
        end_month = quarter * 3
        last_day = calendar.monthrange(year, end_month)[1]
        return datetime.combine(date(year, end_month, last_day) + timedelta(days=1), time.min, UTC)

    calendar_match = CALENDAR_WINDOW_RE.fullmatch(normalized)
    if calendar_match:
        return datetime(int(calendar_match.group("year")) + 1, 1, 1, tzinfo=UTC)

    raise ValueError(f"Unsupported availability window: {value}")
