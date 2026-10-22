"""Referral state checks include transaction-owned cache invalidation."""
import copy
import json

import pytest

from scripts.rust_contract import (
    referral_addresses,
    referral_allocation,
    referral_faults,
    referral_invite,
)


@pytest.fixture(params=["allocation", "retained", "invite", "address"])
def allocation_state(request):
    if request.param == "allocation":
        actor, code = referral_allocation.IDS[1], "VDX-FREE01"
        check = lambda old, new: referral_allocation.only_code(old, new, None, [])
    elif request.param == "retained":
        actor, code = referral_faults.AFTER, "VDX-AFTER1"
        check = lambda old, new: referral_faults.retained(old, new, None, [])
    elif request.param == "invite":
        actor = referral_invite.FIRST
        code = referral_invite.CODES[actor][0]
        check = lambda old, new: referral_invite.keeps_only_code(actor, old, new)
    else:
        actor, code = referral_addresses.SENDER, referral_addresses.CODE
        check = referral_addresses.code_only
    user_key = json.dumps([actor])
    generation_key = json.dumps(["trading"])
    before = {
        "users": {"columns": ["id", "referral_code"], "rows": {
            user_key: {"id": actor, "referral_code": None}}},
        "public_read_cache_generations": {"columns": ["domain", "generation"], "rows": {
            generation_key: {"domain": "trading", "generation": 7}}},
        "public_read_cache_events": {"columns": ["domain", "generation"], "rows": {
            json.dumps(["trading", 7]): {"domain": "trading", "generation": 7}}},
        "notifications": {"columns": ["id"], "rows": {}},
    }
    after = copy.deepcopy(before)
    after["users"]["rows"][user_key]["referral_code"] = code
    after["public_read_cache_generations"]["rows"][generation_key]["generation"] = 8
    after["public_read_cache_events"]["rows"][json.dumps(["trading", 8])] = {
        "domain": "trading", "generation": 8}
    return before, after, check


def test_allocation_accepts_exact_transactional_cache_invalidation(allocation_state):
    before, after, check = allocation_state
    check(before, after)


@pytest.mark.parametrize("damage", ["missing-event", "extra-generation", "unrelated-write", "old-event-change"])
def test_allocation_rejects_incorrect_cache_or_business_changes(allocation_state, damage):
    before, after, check = allocation_state
    if damage == "missing-event":
        del after["public_read_cache_events"]["rows"][json.dumps(["trading", 8])]
    elif damage == "extra-generation":
        after["public_read_cache_generations"]["rows"][json.dumps(["trading"])]["generation"] = 9
    elif damage == "unrelated-write":
        after["notifications"]["rows"]["extra"] = {"id": "extra"}
    else:
        after["public_read_cache_events"]["rows"][json.dumps(["trading", 7])]["domain"] = "changed"
    with pytest.raises(AssertionError):
        check(before, after)
