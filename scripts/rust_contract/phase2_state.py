"""Whole-state differential primitives. No replay after ambiguous commit."""
from urllib.parse import urlsplit

from scripts.rust_contract.compare import compare

# The public read cache checkpoint (`prc_20261018_cache`) puts a statement
# trigger on `users` and on the other trading tables. Each committed writing
# statement advances the `trading` generation by one and records one event row
# for that generation, in the transaction of the write itself.
CACHE_INVALIDATION_TABLES = frozenset({"public_read_cache_generations", "public_read_cache_events"})
CACHE_DOMAIN = "trading"


class StateError(RuntimeError):
    pass


def expect_cache_invalidations(expected, statements):
    """Add the exact metadata produced by committed dependency statements."""
    import json

    if not statements:
        return
    assert CACHE_INVALIDATION_TABLES <= expected.keys(), "cache checkpoint metadata missing"
    rows = expected["public_read_cache_generations"]["rows"]
    generation = rows[json.dumps([CACHE_DOMAIN])]
    events = expected["public_read_cache_events"]["rows"]
    for _ in range(statements):
        generation["generation"] += 1
        number = generation["generation"]
        key = json.dumps([CACHE_DOMAIN, number])
        assert key not in events, "cache event generation already exists"
        events[key] = {"domain": CACHE_DOMAIN, "generation": number}


def validate_cells(python, rust, token, target_url):
    target = urlsplit(target_url)
    if (python["database"] == rust["database"] or not python.get("seed")
            or python["seed"] != rust.get("seed") or not python.get("acl")
            or python["acl"] != rust.get("acl") or len(token) < 32
            or target.scheme != "http" or target.hostname != "127.0.0.1" or not target.port):
        raise StateError("isolated, attested equal-seed loopback cells are required")


def compare_state(python, rust):
    return compare(python, rust)


def unexpected_user_changes(before, after, *, route_id, actor_id, recipient_exists):
    problems = []
    old, new = before.get("users", {}), after.get("users", {})
    if old.keys() != new.keys():
        problems.append("users:membership")
    for user_id in old.keys() & new.keys():
        left, right = dict(old[user_id]), dict(new[user_id])
        allowed = (user_id == actor_id and route_id in {"R01", "R04"}
                   and (route_id != "R04" or not recipient_exists)
                   and not left.get("referral_code"))
        if allowed:
            left.pop("referral_code", None)
            right.pop("referral_code", None)
        problems += [f"users/{user_id}{path}" for path in compare(left, right)]
    return problems


def apply_action(cell, writes, *, fault=None):
    cell.begin()
    try:
        for index, write in enumerate(writes):
            cell.execute(write)
            if fault == "between-writes" and index == 0:
                raise StateError("injected precommit failure")
        if fault == "before-commit":
            raise StateError("injected precommit failure")
        cell.commit()
    except Exception:
        cell.rollback()
        return "rolled-back"
    # The committed unit is retained. Never re-enter execute after this point.
    return "ambiguous" if fault in {"after-commit", "commit-acknowledgement"} else "committed"


def validate_action(action):
    import re
    from datetime import datetime
    from uuid import UUID
    try:
        if set(action) != {"ids", "now", "referral_code", "fault"}:
            raise ValueError("action fields")
        if len(action["ids"]) != 2 or len(set(action["ids"])) != 2:
            raise ValueError("two distinct IDs required")
        for value in action["ids"]:
            UUID(value)
        if datetime.fromisoformat(action["now"]).tzinfo is None:
            raise ValueError("aware action clock required")
        if not re.fullmatch(r"VDX-[A-Z0-9]{6}", action["referral_code"]):
            raise ValueError("referral candidate")
        if action["fault"] not in {"none", "between-writes", "before-commit", "after-commit"}:
            raise ValueError("fault schedule")
    except (KeyError, TypeError, ValueError, AttributeError) as error:
        raise StateError("invalid controlled action") from error
    return action


def require_cleanup(remaining):
    if remaining:
        raise StateError("owned disposable resources remain after cleanup")
