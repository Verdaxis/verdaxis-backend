"""Production-only Telegram monitor for actionable onboarding states."""

import argparse
import asyncio
from datetime import UTC, datetime
import json
import os
from pathlib import Path
import tempfile
from typing import Callable
import urllib.parse
import urllib.request

from app.config import settings
from app.database import AsyncSessionLocal
from app.services.onboarding_attention import (
    OnboardingCandidate,
    classify_candidate,
    format_attention_message,
    format_recovery_message,
    load_candidates,
)


State = dict[str, dict[str, str]]


def load_state(path: Path) -> State:
    if not path.exists():
        return {}
    payload = json.loads(path.read_text(encoding="utf-8"))
    if payload.get("schema_version") != 1 or not isinstance(payload.get("candidates"), dict):
        raise ValueError("invalid onboarding attention state")
    return payload["candidates"]


def write_state(path: Path, state: State) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = {"schema_version": 1, "candidates": state}
    with tempfile.NamedTemporaryFile(
        mode="w", encoding="utf-8", dir=path.parent, delete=False
    ) as handle:
        temporary = Path(handle.name)
        os.chmod(temporary, 0o600)
        json.dump(payload, handle, sort_keys=True, separators=(",", ":"))
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)


def reconcile(
    candidates: list[OnboardingCandidate],
    previous: State,
    now: datetime,
    deliver: Callable[[str], None],
) -> State:
    current = {key: dict(value) for key, value in previous.items()}
    for candidate in candidates:
        attention = classify_candidate(candidate, now)
        prior = previous.get(candidate.key)
        if attention is None:
            if prior is not None:
                if prior.get("silent") != "true":
                    deliver(format_recovery_message(candidate))
                current.pop(candidate.key, None)
            continue
        stage = attention.stage.value
        if prior is not None and prior.get("stage") == stage:
            continue
        deliver(format_attention_message(attention, now))
        current[candidate.key] = {
            "stage": stage,
            "updated_at": now.astimezone(UTC).isoformat(),
        }
    return current


def bootstrap_state(candidates: list[OnboardingCandidate], now: datetime) -> State:
    state: State = {}
    for candidate in candidates:
        attention = classify_candidate(candidate, now)
        if attention is not None:
            state[candidate.key] = {
                "stage": attention.stage.value,
                "updated_at": now.astimezone(UTC).isoformat(),
                "silent": "true",
            }
    return state


def send_telegram(message: str) -> None:
    token = os.environ.get("TELEGRAM_BOT_TOKEN", "").strip()
    chat_id = os.environ.get("TELEGRAM_CHAT_ID", "").strip()
    if not token or not chat_id:
        raise RuntimeError("Telegram configuration is missing")
    base = os.environ.get("TELEGRAM_API_BASE", "https://api.telegram.org").rstrip("/")
    body = urllib.parse.urlencode({"chat_id": chat_id, "text": message}).encode()
    request = urllib.request.Request(
        f"{base}/bot{token}/sendMessage",
        data=body,
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        result = json.loads(response.read())
    if not result.get("ok"):
        raise RuntimeError("Telegram rejected the onboarding alert")


async def run(state_file: Path, *, dry_run: bool, bootstrap: bool) -> int:
    if settings.ENVIRONMENT != "production":
        raise RuntimeError("onboarding attention monitor is production-only")
    async with AsyncSessionLocal() as db:
        candidates = await load_candidates(db)
    now = datetime.now(UTC)
    if dry_run:
        alerts = [
            format_attention_message(attention, now)
            for candidate in candidates
            if (attention := classify_candidate(candidate, now)) is not None
        ]
        print("\n\n".join(alerts) if alerts else "No onboarding attention alerts.")
        return 0

    if bootstrap:
        state = bootstrap_state(candidates, now)
        write_state(state_file, state)
        print(f"onboarding attention baseline recorded {len(state)} current stages")
        return 0

    previous = load_state(state_file)
    current = reconcile(candidates, previous, now, send_telegram)
    write_state(state_file, current)
    print(f"onboarding attention monitor checked {len(candidates)} candidates")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--state-file",
        type=Path,
        default=Path("/var/lib/verdaxis-onboarding-attention/state.json"),
    )
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--bootstrap", action="store_true")
    args = parser.parse_args()
    if args.dry_run and args.bootstrap:
        parser.error("--dry-run and --bootstrap are mutually exclusive")
    return asyncio.run(run(args.state_file, dry_run=args.dry_run, bootstrap=args.bootstrap))


if __name__ == "__main__":
    raise SystemExit(main())
