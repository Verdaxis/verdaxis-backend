"""Lane-owned route schedules; lifecycle and database authority remain in Runner."""
from __future__ import annotations

import hashlib
import json
import random
import threading
import urllib.error
import urllib.request
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path

from scripts.rust_contract.compare import compare_responses
from scripts.rust_contract.phase2_seed import digest
from scripts.rust_contract.phase2_state import compare_state, unexpected_user_changes

ROOT = Path(__file__).resolve().parents[2]
RULES = ROOT / "tests/rust_contract/phase2/normalization.json"


class ScenarioError(RuntimeError):
    pass


@dataclass(frozen=True)
class Action:
    name: str
    route_id: str
    method: str
    path: str
    expected_status: int
    body: dict | None = None
    headers: dict = field(default_factory=dict)
    outcome: str = "committed"
    allow_transport_error: bool = False
    actor_id: str = ""
    recipient_exists: bool = False
    assert_after: Callable | None = None
    mail_status: int = 200
    mail_body: bytes = b'{"id":"disposable-send"}'


@dataclass(frozen=True)
class Server:
    command: tuple[str, ...]
    environment: dict = field(default_factory=dict)


@dataclass(frozen=True)
class Race:
    actors: tuple[Action, ...]
    invariant: Callable
    repeats: int = 3
    seed: int = 20261018


@dataclass(frozen=True)
class Scenario:
    name: str
    group: str
    setup: Callable
    servers: Callable
    actions: Callable
    build: tuple[str, ...] = ()
    races: Callable | None = None
    mixed: Callable | None = None
    dispatch: Callable | None = None
    assert_mixed: Callable | None = None
    route_qualification: bool = True


REGISTRY: dict[str, Scenario] = {}


def register(scenario):
    if scenario.name in REGISTRY:
        raise ScenarioError("duplicate scenario registration")
    REGISTRY[scenario.name] = scenario
    return scenario


def select_scenarios(group, names):
    if names:
        if any(name not in REGISTRY for name in names):
            raise ScenarioError("no registered route scenario for selection")
        selected = [REGISTRY[name] for name in names]
        if group != "all" and any(item.group != group for item in selected):
            raise ScenarioError("scenario does not belong to selected group")
    else:
        selected = [item for item in REGISTRY.values()
                    if item.route_qualification and (group == "all" or item.group == group)]
    if not selected:
        raise ScenarioError("group has no registered route scenario")
    return selected


def _users(state):
    table = state.get("users", {})
    rows = table.get("rows", table)
    return {row.get("id", key): row for key, row in rows.items()}


def _check_result(action, response, error):
    if error:
        if not action.allow_transport_error:
            raise ScenarioError(f"{action.name}: unexpected transport failure {error}")
    elif response["status"] != action.expected_status:
        raise ScenarioError(f"{action.name}: unexpected HTTP status {response['status']}")


def _request(cell, action):
    try:
        return cell.request(action), None
    except (OSError, TimeoutError) as error:
        return None, type(error).__name__


def _compare_http(action, responses, errors):
    if errors[0] != errors[1]:
        raise ScenarioError(f"{action.name}: transport outcomes differ")
    for response, error in zip(responses, errors):
        _check_result(action, response, error)
    if not any(errors):
        wire = [{key: response[key] for key in ("status", "headers", "body")} for response in responses]
        problems = compare_responses(*wire, profile="phase2-meaning-v1", route_id=action.route_id,
                                    rules_sha256=hashlib.sha256(RULES.read_bytes()).hexdigest(),
                                    request_id=action.headers.get("x-request-id"))
        if problems:
            raise ScenarioError(f"{action.name}: HTTP responses differ: {problems}")


def run_actions(actions, cells):
    """Each request is issued once, including ambiguous transport outcomes."""
    evidence = []
    for action in actions:
        before = [cell.snapshot() for cell in cells]
        responses, errors = zip(*[_request(cell, action) for cell in cells])
        after = [cell.snapshot() for cell in cells]
        _compare_http(action, responses, errors)
        if compare_state(*after):
            raise ScenarioError(f"{action.name}: committed state differs")
        mail = [cell.mail() for cell in cells]
        if compare_state(*mail):
            raise ScenarioError(f"{action.name}: captured mail differs")
        for old, new, sends, response in zip(before, after, mail, responses):
            if action.outcome == "rolled-back" and compare_state(old, new):
                raise ScenarioError(f"{action.name}: rollback changed business state")
            if unexpected_user_changes({"users": _users(old)}, {"users": _users(new)},
                                       route_id=action.route_id, actor_id=action.actor_id,
                                       recipient_exists=action.recipient_exists):
                raise ScenarioError(f"{action.name}: unrelated users columns changed")
            if action.assert_after:
                action.assert_after(old, new, response, sends)
        evidence.append({"id": action.name, "outcome": action.outcome,
                         "transport_errors": list(errors), "statuses": [r["status"] if r else None for r in responses],
                         "state_sha256": [digest(state) for state in after],
                         "mail_sha256": [digest(sends) for sends in mail],
                         "database_clock": [response.get("database_clock") if response else None for response in responses]})
    return evidence


def actor_order(actors, seed):
    ordered = list(actors)
    random.Random(seed).shuffle(ordered)
    return ordered


def run_barrier(actors, invoke, *, seed):
    if not 2 <= len(actors) <= 32 or len(set(actors)) != len(actors):
        raise ScenarioError("race requires 2..32 distinct actors")
    barrier = threading.Barrier(len(actors), timeout=20)
    def worker(actor):
        barrier.wait()
        return invoke(actor)
    with ThreadPoolExecutor(max_workers=len(actors)) as pool:
        futures = {actor: pool.submit(worker, actor) for actor in actor_order(actors, seed)}
        return {actor: future.result() for actor, future in futures.items()}


def run_race(race, cells):
    if not 1 <= race.repeats <= 20:
        raise ScenarioError("race repeat count must be 1..20")
    actions = {action.name: action for action in race.actors}
    if len(actions) != len(race.actors):
        raise ScenarioError("race actor names must be unique")
    evidence = []
    for repeat in range(race.repeats):
        seed = race.seed + repeat
        before = [cell.snapshot() for cell in cells]
        results = [run_barrier(tuple(actions), lambda name: _request(cell, actions[name]), seed=seed)
                   for cell in cells]
        after = [cell.snapshot() for cell in cells]
        for name, action in actions.items():
            _compare_http(action, [result[name][0] for result in results], [result[name][1] for result in results])
        if compare_state(*after) or compare_state(cells[0].mail(), cells[1].mail()):
            raise ScenarioError("race committed state or mail differs")
        for old, new, result in zip(before, after, results):
            # Permit only the union of the actors' declared code allocations.
            import copy
            guarded = copy.deepcopy(_users(old))
            new_users = _users(new)
            for action in race.actors:
                row = guarded.get(action.actor_id)
                allowed = (row is not None and not row.get("referral_code")
                           and action.route_id in {"R01", "R04"}
                           and (action.route_id != "R04" or not action.recipient_exists))
                if allowed:
                    row["referral_code"] = new_users.get(action.actor_id, {}).get("referral_code")
            if compare_state(guarded, new_users):
                raise ScenarioError("race changed unrelated users columns")
            race.invariant(old, new, result)
        evidence.append({"seed": seed, "actor_order": actor_order(tuple(actions), seed),
                         "state_sha256": [digest(state) for state in after]})
    return evidence


def run_mixed(schedule, cells, dispatch):
    writers = set()
    for index, action in schedule:
        response, error = _request(cells[index], action)
        _check_result(action, response, error)
        writers.add(index)
    if writers != {0, 1}:
        raise ScenarioError("mixed schedule must exercise both writers")
    return {"dispatcher": dispatch()}


class HTTPCell:
    def __init__(self, endpoint, token, snapshot, mail):
        self.endpoint, self.token = endpoint, token
        self.snapshot, self.mail = snapshot, mail

    def request(self, action):
        if not action.path.startswith("/") or action.path.startswith("//"):
            raise ScenarioError("scenario request must use a local absolute path")
        headers = {"content-type": "application/json", "x-disposable-token": self.token, **action.headers}
        request = urllib.request.Request(self.endpoint + action.path, method=action.method,
            headers=headers, data=None if action.body is None else json.dumps(action.body).encode())
        # Never follow redirects to a different host or resend a write.
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *_args, **_kwargs):
                return None
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        from datetime import UTC, datetime
        before = datetime.now(UTC)
        try:
            response = opener.open(request, timeout=20)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            body = response.read(8 * 1024 * 1024 + 1)
            if len(body) > 8 * 1024 * 1024:
                raise ScenarioError("response exceeds capture bound")
            headers = dict(response.headers)
            clock = None
            if action.path == "/__contract__/phase2/write":
                clock = response.headers.get("x-database-now")
                if not clock or not before <= datetime.fromisoformat(clock) <= datetime.now(UTC):
                    raise ScenarioError("database clock outside captured request interval")
                # This probe-only header is interval evidence, not a route DTO.
                headers = {key: value for key, value in headers.items() if key.lower() != "x-database-now"}
            return {"status": response.status, "headers": headers, "body": body, "database_clock": clock}


# BEGIN LANE REGISTRATIONS: each lane adds one import/register block here.
from scripts.rust_contract.phase2_example import example_scenario

register(example_scenario())
# Phase 2 Task 17–19: referrals (start)
from scripts.rust_contract.phase2_referrals import scenario as referral_scenario

register(referral_scenario())
# Phase 2 Task 17–19: referrals (end)
# END LANE REGISTRATIONS
