"""Declared Phase 2 simulation sizes.

Decision 110 removed Phase 2 speed and memory measurement, so the sizes no
longer describe a measurement workload.  They describe the histories the routes
must page over at correctness scale:

* ``small`` crosses the first full page on every paginated route;
* ``large`` is a long personal history (twelve notification pages, two dozen
  event pages) that proves cursor and offset behaviour does not depend on one
  page of rows.

Every value stays small enough for the runner's full-state snapshot after each
action.  The live caps (five active alerts, one Market Radar, two hundred admin
feedback rows per page) are policy, not fixture size, so a positive fixture
never exceeds them.
"""
from dataclasses import dataclass, fields

SCALED_FIELDS = (
    "notifications",
    "watchlist_events",
    "feedback_history",
    "referrals",
)


class SizeError(ValueError):
    pass


@dataclass(frozen=True)
class SizeProfile:
    name: str
    notifications: int
    watchlist_events: int
    feedback_history: int
    referrals: int
    # Alerts the org already holds before the journey creates one.  The free
    # tier admits five active alerts, and the journey needs one spare slot.
    active_alerts: int
    # Inactive historical rows prove the cap counts active alerts only.
    inactive_alerts: int


PROFILES = {
    "small": SizeProfile(
        name="small",
        notifications=120,
        watchlist_events=60,
        feedback_history=3,
        referrals=12,
        active_alerts=4,
        inactive_alerts=1,
    ),
    "large": SizeProfile(
        name="large",
        notifications=1200,
        watchlist_events=600,
        feedback_history=205,
        referrals=25,
        active_alerts=4,
        inactive_alerts=1,
    ),
}

_NAMES = tuple(PROFILES)


def profile(name: str) -> SizeProfile:
    try:
        return PROFILES[name]
    except KeyError as error:
        raise SizeError(f"unknown Phase 2 size {name!r}; expected one of {_NAMES}") from error


def declared() -> tuple[str, ...]:
    return _NAMES


def histories(value: SizeProfile) -> dict[str, int]:
    """Rows each scenario's volume hook adds, by stored table."""
    return {
        "notifications": value.notifications,
        "watchlist_events": value.watchlist_events,
        "feedback_entries": value.feedback_history,
        "referrals": value.referrals,
    }


def describe(value: SizeProfile) -> dict[str, int | str]:
    """The report shape: the profile's identity and every declared count."""
    document = {"name": value.name}
    for item in fields(SizeProfile):
        if item.name != "name":
            document[item.name] = getattr(value, item.name)
    return document
