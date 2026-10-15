"""Task-local migrated fixture comparison, invoked only by the disposable group."""

import asyncio
import copy
import importlib.util
import json
import os
import re
from datetime import datetime, timedelta
from pathlib import Path
from urllib.parse import quote

from sqlalchemy import text

from app.seeds.safety import seed_session
from scripts.rust_contract.record import differences, record


async def fixtures():
    path = Path("rs/read/tests/market_contract_oracle.py")
    spec = importlib.util.spec_from_file_location("market_oracle", path)
    oracle = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(oracle)
    os.environ["RUST_MARKET_EXPECTED_DATABASE"] = os.environ["SEED_TARGET_DATABASE"]
    async with seed_session() as session:
        cases = await oracle.seed(
            session, datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"])
        )
        assert cases["zero_remaining"]["guard_refused"]
        # Give the two provenance classes different weights and prices.
        for name, quantity, price in [
            ("real_scalar_bid", 10, 100),
            ("demo_ownerless_bid", 30, 200),
        ]:
            await session.execute(
                text(
                    "UPDATE orderbook_orders SET quantity_mt=:quantity, remaining_quantity_mt=:quantity, price_per_mt_usd=:price WHERE id=:id"
                ),
                {
                    "quantity": quantity,
                    "price": price,
                    "id": oracle.fid(f"order:{name}"),
                },
            )
        # A shallow-visible declaration with an invalid required LNG field.
        await session.execute(
            text(
                "UPDATE orderbook_orders SET lng_terms = jsonb_set(lng_terms::jsonb, '{production_country}', '\"ZZ\"')::json WHERE id=:id"
            ),
            {"id": oracle.fid("order:lng_ask_ok")},
        )
        from sqlalchemy import select
        from sqlalchemy.orm import joinedload

        from app.models.orderbook import OrderBookOrder

        # These are shallow-visible but fail full benchmark qualification.
        # Keep this proof separate from the final aggregate comparison.
        from app.services.live_benchmarks import public_slice_order_qualified
        from app.services.public_order_scope import public_order_scope

        instant = datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"])
        filters, joins = public_order_scope(as_of=instant)
        for name in ("typed_invalid_visible", "lng_ask_ok"):
            statement = select(OrderBookOrder).options(
                joinedload(OrderBookOrder.product)
            )
            for target, condition in joins:
                statement = statement.join(target, condition)
            row = (
                await session.execute(
                    statement.where(
                        *filters, OrderBookOrder.id == oracle.fid(f"order:{name}")
                    )
                )
            ).scalar_one()
            assert not public_slice_order_qualified(
                row, as_of=datetime.fromisoformat(os.environ["RUST_CONTRACT_CLOCK"])
            )
        await acceptance_fixtures(session, oracle, instant)
        await session.commit()


WINDOWS = [f"2090-{month:02}" for month in range(1, 13)] + [
    "2090-Q1",
    "2090-CAL",
    "2091-01",
    "2091-02",
]


async def acceptance_fixtures(session, oracle, instant):
    """Clone immutable identities at INSERT, never move an existing slice."""
    from app.schemas.fame_order import FameAskTerms, FameBidTerms

    async def add(name, template, window, **changes):
        source = (
            (
                await session.execute(
                    text("SELECT * FROM orderbook_orders WHERE id=:id"),
                    {"id": oracle.fid(f"order:{template}")},
                )
            )
            .mappings()
            .one()
        )
        fields = set(re.findall(r"(?<!:):([a-z_][a-z_0-9]*)", oracle.INSERT_ORDER))
        values = {key: source[key] for key in fields}
        values.update(
            id=oracle.fid(f"task6a:{name}"),
            availability_window=window,
            quantity_mt="10.00",
            remaining_quantity_mt="10.00",
            price_per_mt_usd="100.00",
        )
        values.update(changes)
        for key in ("certifications", "fame_terms", "lng_terms"):
            if values[key] is not None:
                values[key] = json.dumps(values[key])
        await session.execute(text(oracle.INSERT_ORDER), values)

    # Each bad row changes all three aggregate values if wrongly included.
    for index, field in enumerate(
        ("origin", "specification_standard", "feedstock", "certification_scheme")
    ):
        window = WINDOWS[index]
        await add(
            f"{field}:control", "ask_ok", window, certification_scheme=" custom scheme "
        )
        for code in range(0x1C, 0x20):
            await add(
                f"{field}:{code}",
                "ask_ok",
                window,
                price_per_mt_usd="200.00",
                **{field: chr(code)},
            )

    terms = json.loads(Path("tests/rust_contract/vectors/terms/fame.json").read_text())[
        "row"
    ]["fame_terms"]
    for decimal in ("1E-29", "0E-100"):
        payload = copy.deepcopy(terms)
        payload["quality_evidence"]["results"][0]["value"] = decimal
        stored = FameAskTerms.model_validate(payload).model_dump(mode="json")
        assert stored["quality_evidence"]["results"][0]["value"] == decimal
        await add(f"decimal:{decimal}", "fame_ask_ok", WINDOWS[4], fame_terms=stored)

    payload = copy.deepcopy(terms)
    payload["certificate_valid_until"] = "2026-10-15"
    await add("midnight-certificate", "fame_ask_ok", WINDOWS[5], fame_terms=payload)
    for field in ("sampled_on", "tested_on"):
        payload = copy.deepcopy(terms)
        payload["certificate_valid_until"] = "2099-01-01"
        payload["quality_evidence"]["sampled_on"] = "2026-10-15"
        payload["quality_evidence"]["tested_on"] = "2026-10-16"
        if field == "sampled_on":
            # PENDING evidence permits an absent test date. This isolates the
            # sampling-date rule instead of masking it with a future test date.
            payload["quality_evidence"]["status"] = "PENDING"
            payload["quality_evidence"]["sampled_on"] = "2026-10-16"
            payload["quality_evidence"]["tested_on"] = None
        await add(f"quality:{field}", "fame_ask_ok", WINDOWS[6], fame_terms=payload)

    for microseconds in (-1, 0, 1):
        await add(
            f"expiry:{microseconds}",
            "real_scalar_bid",
            WINDOWS[7],
            expires_at=instant + timedelta(microseconds=microseconds),
        )
    for name in (
        "grace_before",
        "grace_edge",
        "grace_after",
        "pending_no_grace",
        "submitted_no_grace",
        "foreign_review_grace",
        "rejected_with_grace",
    ):
        template = "grace_edge_bid" if name == "grace_edge" else name
        await add(f"kyc:{name}", template, WINDOWS[8])
    for index, prices in enumerate((("100.00", "100.01"), ("100.01", "100.02"))):
        for number, price in enumerate(prices):
            await add(
                f"round:{index}:{number}",
                "real_scalar_bid",
                WINDOWS[9 + index],
                price_per_mt_usd=price,
            )
    await add(
        "partial",
        "partial_bid",
        WINDOWS[11],
        quantity_mt="100.00",
        remaining_quantity_mt="40.00",
    )
    await add("quarter", "real_scalar_bid", WINDOWS[12])
    await add("calendar", "real_scalar_bid", WINDOWS[13])
    await add(
        "typed-bid",
        "fame_ask_ok",
        "2091-01",
        side="BID",
        fame_terms=FameBidTerms.model_validate(
            {
                "side": "BID",
                "neat_fame": True,
                "standard": "EN_14214",
                "standard_edition": "EN 14214:2019",
                "sustainability_scheme": "ISCC_EU",
            }
        ).model_dump(mode="json"),
    )
    valid_lng = (
        await session.execute(
            text("SELECT lng_terms FROM orderbook_orders WHERE id=:id"),
            {"id": oracle.fid("order:lng_ask_reseller")},
        )
    ).scalar_one()
    await add(
        "lng-null-ci",
        "lng_ask_reseller",
        "2091-02",
        lng_terms=valid_lng,
        carbon_intensity_gco2_mj=None,
    )
    # Current guards refuse wrong-lane inserts; never weaken them for a test.
    source = (
        await session.execute(
            text("SELECT lng_terms FROM orderbook_orders WHERE id=:id"),
            {"id": oracle.fid("order:lng_ask_reseller")},
        )
    ).scalar_one()
    point = (
        await session.execute(
            text("SELECT delivery_point_id FROM orderbook_orders WHERE id=:id"),
            {"id": oracle.fid("order:real_scalar_bid")},
        )
    ).scalar_one()
    from sqlalchemy.exc import IntegrityError

    try:
        async with session.begin_nested():
            await add(
                "wrong-lane",
                "lng_ask_reseller",
                WINDOWS[13],
                delivery_point_id=point,
                lng_terms=source,
            )
    except IntegrityError as error:
        assert "ck_orderbook_orders_lng_terms" in str(error)
    else:
        raise AssertionError("wrong-lane LNG insert should be refused")


def acceptance_cases(token):
    base = os.environ["RUST_CONTRACT_CLOCK"]
    cases = []
    for gate in ("true", "false"):
        for name, instant in (
            ("base", base),
            ("before", "2026-10-15T15:59:59.999999Z"),
            ("after", "2026-10-15T16:00:00Z"),
        ):
            cases.append(
                {
                    "id": f"acceptance-{gate}-{name}",
                    "method": "GET",
                    "path": f"/__contract__/live-benchmarks?gate={gate}&as_of={quote(instant, safe='')}&windows={','.join(WINDOWS)}",
                }
            )
    cases.append(
        {
            "id": "acceptance-cache-expansion",
            "method": "GET",
            "path": f"/__contract__/live-benchmarks?expand=true&windows={WINDOWS[0]}",
        }
    )
    expected = record(os.environ["PYTHON_CONTRACT_URL"], token, cases)
    actual = record(os.environ["RUST_CONTRACT_URL"], token, cases)
    assert not differences(expected, actual), differences(expected, actual)
    for case, result in zip(cases[:-1], expected[:-1]):
        populated = {
            row["window"]: row
            for row in result["exchange"]["body"]
            if row["count"] is not None
        }

        def aggregate(window, price, quantity, count, populated=populated, case=case):
            row = populated[window]
            assert (row["price"], row["quantity"], row["count"]) == (
                price,
                quantity,
                count,
            ), (case["id"], row)

        for window in WINDOWS[:4]:
            aggregate(window, "100.00", "10.00", 1)
        aggregate(WINDOWS[4], "100.00", "20.00", 2)
        aggregate(WINDOWS[9], "100.00", "20.00", 2)
        aggregate(WINDOWS[10], "100.02", "20.00", 2)
        aggregate(WINDOWS[11], "100.00", "40.00", 1)
        aggregate(WINDOWS[12], "100.00", "10.00", 1)
        aggregate(WINDOWS[13], "100.00", "10.00", 1)
        aggregate(WINDOWS[14], "100.00", "10.00", 1)
        aggregate(WINDOWS[15], "100.00", "10.00", 1)
        if case["id"].endswith("base"):
            aggregate(WINDOWS[7], "100.00", "10.00", 1)
            count = 2 if "true" in case["id"] else 5
            aggregate(WINDOWS[8], "100.00", f"{count * 10}.00", count)
        if case["id"].endswith("before"):
            aggregate(WINDOWS[5], "100.00", "10.00", 1)
            assert WINDOWS[6] not in populated
        if case["id"].endswith("after"):
            assert WINDOWS[5] not in populated
            aggregate(WINDOWS[6], "100.00", "20.00", 2)


def main():
    asyncio.run(fixtures())
    cases = [
        {"id": "matrix-all", "method": "GET", "path": "/__contract__/live-benchmarks"},
        {
            "id": "matrix-sparse",
            "method": "GET",
            "path": "/__contract__/live-benchmarks?selection=sparse",
        },
        {
            "id": "matrix-expiry",
            "method": "GET",
            "path": "/__contract__/live-benchmarks?as_of=2026-10-16T12:00:00Z",
        },
    ]
    token = os.environ["DISPOSABLE_TEST_TOKEN"]
    expected = record(os.environ["PYTHON_CONTRACT_URL"], token, cases)
    actual = record(os.environ["RUST_CONTRACT_URL"], token, cases)
    assert any(row["price"] is not None for row in expected[0]["exchange"]["body"])
    problems = differences(expected, actual)
    assert not problems, problems
    acceptance_cases(token)
    print("live benchmark matrix: 3 base cases and 7 acceptance cases matched")


if __name__ == "__main__":
    main()
