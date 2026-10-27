"""The leaderboard's count-only Python order does not define tie membership."""
import copy
import importlib
import json

import pytest


def response(names):
    entries = [{"rank": i + 1, "user_name": name, "organization_name": None,
                "referral_count": count} for i, (name, count) in enumerate(names)]
    return {"status": 200, "headers": {"content-type": "application/json"},
            "body": json.dumps(entries).encode()}


def compare(left, right, route="R03"):
    runner = importlib.import_module("scripts.rust_contract.phase2_scenarios")
    action = runner.Action("leaderboard", route, "GET", "/api/referrals/leaderboard", 200)
    runner._compare_http(action, [left, right], [None, None])


def test_complete_ties_compare_members_without_rank_assignment():
    compare(response([("A", 3), ("B", 2), ("C", 2)]),
            response([("A", 3), ("C", 2), ("B", 2)]))


def test_cut_tie_allows_different_distinct_members():
    prefix = [(str(i), 20 - i) for i in range(8)]
    compare(response(prefix + [("A", 2), ("B", 2)]),
            response(prefix + [("C", 2), ("D", 2)]))


@pytest.mark.parametrize("damage", ["count", "duplicate", "rank", "whole-group", "header", "status"])
def test_tie_rule_preserves_other_contract_checks(damage):
    left = response([("A", 3), ("B", 2), ("C", 2)])
    right = copy.deepcopy(left)
    entries = json.loads(right["body"])
    if damage == "count":
        entries[1]["referral_count"] = 1
    elif damage == "duplicate":
        entries[2]["user_name"] = "B"
    elif damage == "rank":
        entries[1]["rank"] = 9
    elif damage == "whole-group":
        entries[2]["user_name"] = "D"
    elif damage == "header":
        right["headers"]["x-contract"] = "changed"
    else:
        right["status"] = 201
    right["body"] = json.dumps(entries).encode()
    with pytest.raises((AssertionError, RuntimeError)):
        compare(left, right)


@pytest.mark.parametrize("rank", [True, 1.0, "1"], ids=["boolean", "float", "string"])
def test_rank_requires_an_integer(rank):
    left = response([("A", 3)])
    right = copy.deepcopy(left)
    entries = json.loads(right["body"])
    entries[0]["rank"] = rank
    right["body"] = json.dumps(entries).encode()
    with pytest.raises((AssertionError, RuntimeError)):
        compare(left, right)


def test_other_routes_keep_ordered_comparison():
    with pytest.raises((AssertionError, RuntimeError)):
        compare(response([("A", 3), ("B", 2), ("C", 2)]),
                response([("A", 3), ("C", 2), ("B", 2)]), route="R02")
