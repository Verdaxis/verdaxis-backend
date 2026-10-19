"""Assertion and request-control helpers shared by the watchlist write scenarios."""
import json
from datetime import datetime

from scripts.rust_contract.phase2_scenarios import ScenarioError
from scripts.rust_contract.phase2_watchlist_fixtures import at, bearer


def uid(group, number):
    return f"{group}000000-0000-4000-8000-{number:012d}"


def rows(state, table):
    return list(state[table]["rows"].values())


def unchanged_except(before, after, *tables):
    for table in before:
        if table not in tables and before[table] != after[table]:
            raise ScenarioError(f"unexpected write: {table}")


def created(before, after, table):
    known = {row["id"] for row in rows(before, table)}
    return [row for row in rows(after, table) if row["id"] not in known]


def same_instant(stored, expected):
    return datetime.fromisoformat(stored) == datetime.fromisoformat(expected)


def body_of(response):
    return json.loads(response["body"])


def no_write(before, after, response, mail):
    if before != after or mail:
        raise ScenarioError("refused request changed state or sent mail")


class Schedule:
    """One clock tick and one identifier list per request, shared by both stacks."""

    def __init__(self, secret, request_id, day=2):
        self.secret = secret
        self.request_id = request_id
        self.day = day
        self.tick = 0

    def headers(self, user, ids, *, fail_summary=False, token=None):
        self.tick += 1
        clock = at(days=self.day, minutes=self.tick)
        headers = {"authorization": token or bearer(self.secret, user), "x-request-id": self.request_id,
                   "x-test-watchlist-clock": clock.isoformat(),
                   "x-test-watchlist-ids": ",".join(ids)}
        if fail_summary:
            headers["x-test-watchlist-fail-summary"] = "true"
        return headers, clock.isoformat()
