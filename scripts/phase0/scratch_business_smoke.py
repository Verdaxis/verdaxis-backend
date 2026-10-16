"""Scratch-only business smoke: an external HTTP client for a rehearsal backend.

This is the bounded adapter required before Task 10's P0-5 rehearsal. It does not
change the Task 9 harness or its disposable-target gate. It refuses any base URL
that could be the live service, creates scratch-only identities through a
separate fixture connection, then proves that real application business commands
and copied public reads work against the reviewed release.

Run it only against a scratch cluster (port 5434) and scratch backend
(127.0.0.1:8100). Never point it at staging or production. No secret is written
to argv, stdout or the report.
"""
from __future__ import annotations

import argparse
import asyncio
import ipaddress
import os
import re
import secrets
import uuid
from decimal import Decimal
from pathlib import Path
from urllib.parse import urlsplit

import httpx
from sqlalchemy import func, select, text
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.core.security import get_password_hash
from app.market_catalog import (
    B30_SPECIFICATION_STANDARD,
    CANONICAL_DELIVERY_POINT_IDS,
    CANONICAL_PRODUCTS,
)
from app.models.orderbook import OrderBookOrder, OrderSide
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.services.public_order_scope import public_order_scope
from scripts.phase0.copy import private_json, read_private_file

# The live prod cluster listens on 5432; P0-5 gives the rehearsal its own cluster.
LIVE_DATABASE_PORTS = {5432}
# The live backend listeners; a scratch backend must never reuse them.
LIVE_BACKEND_PORTS = {8000, 8001}
TARGET_DATABASE = "verdaxis_prod"
# The rehearsal uses the exact target name. The adapter's own disposable test
# keeps the app's test-environment rule, so the same *_test form is allowed here.
DISPOSABLE_TEST_DATABASE = re.compile(r"^[a-z][a-z0-9_]*_test$")
# B30 has no restricted delivery-point lane, so it is safe on any copied catalog.
PRODUCT_CODE = "B30"
PASSWORD_BYTES = 24
# The public order-book sides and their paths, in the order the smoke reports them.
PUBLIC_SIDES = (("BID", "/api/orderbook/bids"), ("ASK", "/api/orderbook/asks"))
# The smoke itself places one BID and one ASK; the public path also serves those.
SMOKE_OWN_ORDERS = {"BID": 1, "ASK": 1}
# The smoke proves that copied rows the public rules make visible are served, so
# it samples only orders the application's own public predicate admits and needs
# at least this many, or an empty (or fully hidden) copy is visible as a failure.
COPIED_ORDER_SAMPLE = 3
# Bounded page walk for the sampled orders; newest-first sampling finds them on
# the first page, so the cap only bounds a pathological copy.
SAMPLE_PAGE_SIZE = 100
SAMPLE_MAX_PAGES = 6


def require_scratch_base_url(value: str) -> str:
    """Accept only a plain numeric-loopback URL on a non-live port."""
    parts = urlsplit(value)
    if parts.scheme != "http" or parts.username or parts.password:
        raise ValueError("base URL must be plain http with no userinfo")
    if parts.path not in ("", "/") or parts.query or parts.fragment:
        raise ValueError("base URL must not contain a path, query or fragment")
    try:
        host = ipaddress.ip_address(parts.hostname or "")
    except ValueError:
        raise ValueError("base URL host must be a literal IP address") from None
    if not host.is_loopback:
        raise ValueError("base URL must be a loopback address")
    try:
        port = parts.port
    except ValueError:
        raise ValueError("base URL port must be numeric") from None
    if port is None or not 1024 < port < 65536:
        raise ValueError("base URL port must be an unprivileged numeric port")
    if port in LIVE_BACKEND_PORTS:
        raise ValueError("base URL port is a live backend listener")
    return f"http://{host}:{port}"


def read_fixture_url(path: Path):
    """Read the one-line scratch fixture URL. Never a live database."""
    lines = read_private_file(path).splitlines()
    if len(lines) != 1 or not lines[0].startswith("DATABASE_URL="):
        raise ValueError("fixture file needs exactly one DATABASE_URL line")
    url = make_url(lines[0][len("DATABASE_URL="):])
    if url.drivername != "postgresql+asyncpg" or url.query or not url.password:
        raise ValueError("fixture URL must be a credentialled asyncpg URL")
    if not url.host or not ipaddress.ip_address(url.host).is_loopback:
        raise ValueError("fixture URL must use a loopback host")
    # A missing port defaults to 5432, which is the live cluster, so an
    # explicit port is required before the live-port refusal can apply.
    if url.port is None:
        raise ValueError("fixture URL must name its port explicitly")
    if url.port in LIVE_DATABASE_PORTS:
        raise ValueError("fixture URL points at the live cluster port")
    if url.database != TARGET_DATABASE and not DISPOSABLE_TEST_DATABASE.fullmatch(url.database or ""):
        raise ValueError("fixture URL must name the scratch target database")
    return url


class SmokeFailed(RuntimeError):
    """A business case failed; stop and keep the report."""


class Report:
    """Ordered case results; a failure never hides an earlier pass."""

    def __init__(self) -> None:
        self.cases: list[dict] = []

    def record(self, case: str, status: int | str, passed: bool, detail: str = "") -> None:
        # Detail carries field names and codes only, never a secret or a body.
        self.cases.append(dict(case=case, status=status,
                               result="pass" if passed else "fail", detail=detail))

    @staticmethod
    def _reason(response) -> str:
        """A stable product detail code only; never a body, row or secret.

        Non-string or oversized details are replaced with a class marker, so an
        unexpected error page cannot copy data into the report.
        """
        try:
            value = response.json()
        except ValueError:
            return "non-json-response"
        if isinstance(value, dict) and isinstance(value.get("detail"), str) and len(value["detail"]) <= 200:
            return value["detail"]
        return "unstructured-response"

    def require(self, case: str, response, expected: int, detail: str = "") -> dict:
        try:
            body = response.json()
        except ValueError:
            body = {}
        passed = response.status_code == expected
        if passed:
            self.record(case, response.status_code, True, detail)
        else:
            self.record(case, response.status_code, False, self._reason(response))
            raise SmokeFailed(f"{case} returned {response.status_code}, expected {expected}")
        return body


async def discover_port(session) -> str:
    value = (await session.execute(text(
        "SELECT id FROM ports ORDER BY id LIMIT 1"))).scalar_one_or_none()
    if value is None:
        raise SmokeFailed("scratch target has no copied port")
    return str(value)


async def discover_lane(session) -> tuple[str, str, str]:
    """Use canonical catalog identity, verified against the copied database."""
    spec = next(product for product in CANONICAL_PRODUCTS
                if product.market_product.value == PRODUCT_CODE)
    product = (await session.execute(text("SELECT id FROM products WHERE id = :id"),
                                     {"id": str(spec.id)})).scalar_one_or_none()
    if product is None:
        raise SmokeFailed("scratch target lacks the canonical smoke product")
    for candidate in spec.available_delivery_point_ids or CANONICAL_DELIVERY_POINT_IDS:
        found = (await session.execute(text("SELECT id FROM delivery_points WHERE id = :id"),
                                       {"id": str(candidate)})).scalar_one_or_none()
        if found is not None:
            return str(product), str(found), spec.fuel_type
    raise SmokeFailed("scratch target lacks the canonical smoke delivery point")


def side_name(value: OrderSide | str) -> str:
    """Return the two-letter side name for an enum member or a raw string."""
    return value.value if isinstance(value, OrderSide) else str(value)


async def visible_copied_orders(session) -> tuple[list[dict], dict[str, int]]:
    """Sample copied orders with the application's own public visibility rules.

    The predicate is ``public_order_scope``, the same one the public order-book
    routes use, so the smoke never restates the rules. It returns the newest
    visible copies in the routes' own newest order plus the per-side visible
    counts, so the caller can check each sampled order and the served totals.
    """
    filters, joins = public_order_scope()
    counts = {side: 0 for side, _ in PUBLIC_SIDES}
    count_query = select(OrderBookOrder.side, func.count(OrderBookOrder.id))
    sample_query = select(OrderBookOrder.id, OrderBookOrder.side)
    for target, condition in joins:
        count_query = count_query.join(target, condition)
        sample_query = sample_query.join(target, condition)
    for side, total in (await session.execute(
            count_query.where(*filters).group_by(OrderBookOrder.side))).all():
        counts[side_name(side)] = int(total)
    sample = [
        dict(id=str(identifier), side=side_name(side))
        for identifier, side in (await session.execute(
            sample_query.where(*filters)
            .order_by(OrderBookOrder.created_at.desc(), OrderBookOrder.id.desc())
            .limit(COPIED_ORDER_SAMPLE))).all()
    ]
    return sample, counts


async def create_scratch_identities(url) -> dict:
    """Create two scratch organizations: one supplier and one buyer, plus admin.

    The buyer must be in a different organization from the seller, or the trade
    route refuses a self-trade. The fixture connection must be the scratch
    cluster administrator: the provenance marker needs the replication-role
    switch, exactly as the existing disposable test helper does. Provenance is
    written only in the scratch target, never through the runtime application
    role and never on the live cluster (the port guard refuses that first).
    """
    password = secrets.token_urlsafe(PASSWORD_BYTES)
    token = uuid.uuid4().hex[:12]
    engine = create_async_engine(url, hide_parameters=True)
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as session:
            role = (await session.execute(text(
                "SELECT rolsuper FROM pg_roles WHERE rolname=current_user"))).scalar_one_or_none()
            database = (await session.execute(text("SELECT current_database()"))).scalar_one()
            scratch_names = {TARGET_DATABASE}
            if database not in scratch_names and not DISPOSABLE_TEST_DATABASE.fullmatch(database):
                raise SmokeFailed("scratch fixture is not the rehearsal target")
            if role is not True:
                raise SmokeFailed("scratch fixture needs the scratch administrator role")
            port = await discover_port(session)
            product_id, delivery_point_id, fuel_type = await discover_lane(session)
            supplier_org = Organization(
                name=f"P0 scratch smoke supplier {token}", type=OrgType.FUEL_SUPPLIER,
                verification_status="APPROVED",
            )
            buyer_org = Organization(
                name=f"P0 scratch smoke buyer {token}", type=OrgType.FUEL_BUYER,
                verification_status="APPROVED",
            )
            session.add_all([supplier_org, buyer_org])
            await session.flush()
            identities: dict[str, dict] = {}
            for kind, user_role, organization in (
                    ("supplier", UserRole.SUPPLIER, supplier_org),
                    ("buyer", UserRole.BUYER, buyer_org),
                    ("admin", UserRole.ADMIN, supplier_org)):
                email = f"p0-scratch-{kind}-{token}@example.invalid"
                user = User(
                    id=uuid.uuid4(), email=email,
                    password_hash=get_password_hash(password),
                    first_name="Phase0", last_name=f"Scratch {kind}", role=user_role,
                    status=UserStatus.APPROVED, email_verified=True,
                    organization=organization, kyc_status="APPROVED",
                )
                session.add(user)
                await session.flush()
                user.kyc_organization_id = organization.id
                identities[kind] = dict(email=email, password=password, id=str(user.id),
                                        organization_id=str(organization.id))
            copied_orders, copied_visible_counts = await visible_copied_orders(session)
            # Mark both organizations REAL as the disposable helper does, bounded
            # to this scratch target rather than a *_test database.
            await session.execute(text("SET LOCAL session_replication_role = 'replica'"))
            await session.execute(text(
                "UPDATE organizations SET provenance = 'REAL' "
                "WHERE id IN (:supplier, :buyer)"),
                {"supplier": str(supplier_org.id), "buyer": str(buyer_org.id)})
            await session.commit()
    finally:
        await engine.dispose()
    return dict(identities=identities, port=port, product_id=product_id,
                delivery_point_id=delivery_point_id, fuel_type=fuel_type,
                copied_orders=copied_orders,
                copied_visible_counts=copied_visible_counts)


def run_cases(base_url: str, fixture: dict, report: Report) -> None:
    """Run the bounded business commands as an external HTTP client."""
    headers: dict[str, dict] = {}
    with httpx.Client(base_url=base_url, timeout=20.0) as client:

        def request(case: str, method: str, path: str, expected: int, **kwargs) -> dict:
            return report.require(case, client.request(method, path, **kwargs), expected)

        def public_total(path: str) -> int:
            """Read one public side's total with a bounded request."""
            response = client.get(path, params={"limit": 1})
            if response.status_code != 200:
                raise SmokeFailed(f"public read {path} returned {response.status_code}")
            total = response.json().get("total")
            if not isinstance(total, int):
                raise SmokeFailed(f"public read {path} has no integer total")
            return total

        def served_ids(path: str, wanted: set[str]) -> set[str]:
            """Walk bounded public pages and return the ids that were served."""
            listed: set[str] = set()
            for index in range(SAMPLE_MAX_PAGES):
                response = client.get(path, params={"limit": SAMPLE_PAGE_SIZE,
                                                    "skip": index * SAMPLE_PAGE_SIZE})
                if response.status_code != 200:
                    raise SmokeFailed(f"public read {path} returned {response.status_code}")
                rows = response.json().get("items", [])
                listed.update(str(row.get("id")) for row in rows)
                if wanted <= listed or len(rows) < SAMPLE_PAGE_SIZE:
                    break
            return listed

        for kind, identity in fixture["identities"].items():
            body = request(f"login:{kind}", "POST", "/api/auth/login", 200,
                           data={"username": identity["email"],
                                 "password": identity["password"]})
            if "access_token" not in body:
                raise SmokeFailed(f"login:{kind} returned no access token")
            headers[kind] = {"Authorization": f"Bearer {body['access_token']}"}
        me = request("me:supplier", "GET", "/api/auth/me", 200, headers=headers["supplier"])
        if (me.get("email") != fixture["identities"]["supplier"]["email"]
                or me.get("role") != UserRole.SUPPLIER.value):
            raise SmokeFailed("me:supplier identity differs")

        request("inventory:create", "POST", "/api/inventory", 200, headers=headers["supplier"],
                json={"port_id": fixture["port"], "fuel_type": fixture["fuel_type"],
                      "product_name": PRODUCT_CODE, "current_stock_mt": 1000,
                      "price_per_mt_usd": 900, "certification_declared": True,
                      "certification_scheme": "ISCC EU"})
        lane = {"product_id": fixture["product_id"],
                "delivery_point_id": fixture["delivery_point_id"],
                "availability_window": "Spot"}
        # An ASK must declare its supplier details; the API refuses a bare ASK.
        supplier_details = {
            "certification_declared": True, "certification_scheme": "ISCC EU",
            "specification_standard": B30_SPECIFICATION_STANDARD, "msds_available": True,
            "carbon_intensity_gco2_mj": "20.5",
            "carbon_intensity_method": "ISCC EU default pathway",
            "feedstock": "Waste-based", "origin": "Singapore",
        }
        ask = request("order:ask", "POST", "/api/orderbook", 201, headers=headers["supplier"],
                      json={**lane, "side": "ASK", "quantity_mt": 500,
                            "price_per_mt_usd": 1200, **supplier_details})
        # A lower BID price is not executable against the ASK, so inserting the
        # BID cannot auto-match and consume the order this smoke later takes.
        request("order:bid", "POST", "/api/orderbook", 201, headers=headers["buyer"],
                json={**lane, "side": "BID", "quantity_mt": 200, "price_per_mt_usd": 1100})
        if not ask.get("terms_digest") or not ask.get("id"):
            raise SmokeFailed("order:ask returned no terms digest")
        trade = request("trade:hit", "POST", "/api/trades/", 200, headers=headers["buyer"],
                        json={"order_id": ask["id"], "quantity_mt": 200,
                              "expected_terms_digest": ask["terms_digest"]})
        trade_id = trade.get("id")
        if not trade_id:
            raise SmokeFailed("trade:hit returned no trade id")
        confirmed = request("trade:confirm", "PUT", f"/api/trades/{trade_id}/confirm", 200,
                            headers=headers["supplier"])
        if confirmed.get("status") != "CONFIRMED":
            raise SmokeFailed("trade:confirm status differs")
        delivered = request("trade:deliver", "PUT", f"/api/trades/{trade_id}/deliver", 200,
                            headers=headers["supplier"],
                            json={"final_quantity_mt": "200", "final_price_per_mt": "1200"})
        if delivered.get("status") != "DELIVERED":
            raise SmokeFailed("trade:deliver status differs")
        if Decimal(str(delivered.get("final_total_usd", "0"))) != Decimal(240000):
            raise SmokeFailed("trade:deliver total differs")
        paid = request("trade:pay", "POST", f"/api/trades/{trade_id}/pay", 200,
                       headers=headers["supplier"])
        if paid.get("status") != "PAID":
            raise SmokeFailed("trade:pay status differs")

        notifications = request("notifications:list", "GET", "/api/notifications", 200,
                                headers=headers["supplier"])
        if not isinstance(notifications, list) or not notifications:
            raise SmokeFailed("trade lifecycle produced no notification")
        request("notifications:read", "PATCH",
                f"/api/notifications/{notifications[0]['id']}/read", 200,
                headers=headers["supplier"])
        request("orderbook:mine:supplier", "GET", "/api/orderbook/my", 200,
                headers=headers["supplier"])
        request("orderbook:mine:buyer", "GET", "/api/orderbook/my", 200, headers=headers["buyer"])

        public = request("public:orderbook", "GET", "/api/orderbook", 200)
        request("public:prices", "GET", "/api/prices", 200)
        rows = public if isinstance(public, list) else public.get("items", public.get("orders", []))
        if not isinstance(rows, list):
            raise SmokeFailed("public orderbook shape differs")
        if not any(str(row.get("id")) == ask["id"] for row in rows):
            raise SmokeFailed("public orderbook does not serve the new ASK")
        # Copied rows must be served exactly as the public rules make them
        # visible. The expected set and counts come from the application's own
        # public predicate, so a hidden copied row can never be sampled and an
        # empty or fully hidden copy is still visible as a failure. The per-side
        # totals also catch a copied order that is served on no page at all.
        copied = fixture.get("copied_orders") or []
        if len(copied) < COPIED_ORDER_SAMPLE:
            report.record("public:copied-orders", len(copied), False,
                          f"visible_copied={len(copied)}")
            raise SmokeFailed(
                f"fewer than {COPIED_ORDER_SAMPLE} copied orders are publicly visible")
        expected = {side: fixture["copied_visible_counts"].get(side, 0) + own
                    for side, own in SMOKE_OWN_ORDERS.items()}
        observed: dict[str, int] = {}
        served: set[str] = set()
        for side, path in PUBLIC_SIDES:
            observed[side] = public_total(path)
            wanted = {row["id"] for row in copied if row["side"] == side}
            if wanted:
                served |= served_ids(path, wanted)
        missing = [row["id"] for row in copied if row["id"] not in served]
        detail = (f"visible_copied={len(copied)} served={len(copied) - len(missing)} "
                  f"bids={observed['BID']}/{expected['BID']} asks={observed['ASK']}/{expected['ASK']}")
        passed = not missing and observed == expected
        report.record("public:copied-orders", len(copied), passed, detail)
        if not passed:
            raise SmokeFailed("public orderbook does not serve the visible copied orders")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--fixture-environment-file", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    report = Report()
    try:
        base_url = require_scratch_base_url(args.base_url)
        if not args.report.is_absolute() or args.report.exists():
            raise ValueError("report must be a new absolute path")
        url = read_fixture_url(args.fixture_environment_file)
        fixture = asyncio.run(create_scratch_identities(url))
        run_cases(base_url, fixture, report)
    except Exception as error:
        # Bounded diagnostics: no URL, password, token or response body. The
        # traceback is code frames only, and only when an operator asks for it.
        if os.environ.get("PHASE0_SMOKE_TRACEBACK") == "1":
            import traceback
            traceback.print_exc()
        report.record("adapter", "error", False, type(error).__name__)
        private_json(args.report, dict(cases=report.cases, passed=False))
        print(f"scratch business smoke failed: {type(error).__name__}; see the report")
        return 1
    private_json(args.report, dict(cases=report.cases, passed=True))
    print(f"scratch business smoke passed: {len(report.cases)} cases verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
