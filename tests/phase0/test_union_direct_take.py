"""Real authenticated scalar-biofuel takes must pass the union SQL guard."""
from decimal import Decimal
from uuid import uuid4

import httpx
import pytest
from sqlalchemy import select
from sqlalchemy.ext.asyncio import async_sessionmaker

from app.core.security import create_access_token
from app.database import get_db
from app.main import app
from app.market_catalog import PRODUCTS_BY_CODE, DELIVERY_POINTS_BY_NAME
from app.models.orderbook import OrderBookOrder, Trade
from app.models.user import Organization, User
from app.services.order_terms import order_terms_digest
from tests.phase0.test_copy import copy_case as copy_case


@pytest.mark.asyncio
@pytest.mark.parametrize('product_code', ['B30', 'B100'])
async def test_union_scalar_biofuel_direct_take(copy_case, product_code):
    _, _, target, _ = copy_case
    factory = async_sessionmaker(target.engine, expire_on_commit=False)
    product = PRODUCTS_BY_CODE[product_code]
    point = DELIVERY_POINTS_BY_NAME['Singapore']
    async with factory() as session:
        buyer_org = Organization(id=uuid4(), name='Synthetic take buyer', type='FUEL_BUYER',
                                 verification_status='APPROVED', provenance='REAL')
        seller_org = Organization(id=uuid4(), name='Synthetic take seller', type='FUEL_SUPPLIER',
                                  verification_status='APPROVED', provenance='REAL')
        buyer = User(id=uuid4(), email=f'{uuid4()}@example.invalid', password_hash='unused',
                     organization=buyer_org, role='BUYER', status='APPROVED', email_verified=True,
                     kyc_status='APPROVED', kyc_organization_id=buyer_org.id)
        seller = User(id=uuid4(), email=f'{uuid4()}@example.invalid', password_hash='unused',
                      organization=seller_org, role='SUPPLIER', status='APPROVED', email_verified=True,
                      kyc_status='APPROVED', kyc_organization_id=seller_org.id)
        session.add_all([buyer, seller])
        await session.flush()
        order = OrderBookOrder(id=uuid4(), organization_id=seller_org.id, owner_user_id=seller.id,
            product_id=product.id, delivery_point_id=point.id, side='ASK', status='OPEN',
            quantity_mt=Decimal('400'), remaining_quantity_mt=Decimal('400'), price_per_mt_usd=Decimal('700'),
            availability_window='SPOT', provenance='REAL', certification_declared=True,
            certification_scheme='ISCC EU', msds_available=True, carbon_intensity_gco2_mj=Decimal('20'),
            carbon_intensity_method='Whole supplied fuel lifecycle', feedstock='Used cooking oil',
            origin='Netherlands', off_spec=False,
            specification_standard='ISO 8217:2024 RF 380' if product_code == 'B30' else 'ISO 8217:2024 DFA')
        session.add(order)
        await session.commit()
        digest = order_terms_digest(order)
        order_id, buyer_id = order.id, buyer.id

    async def override_db():
        async with factory() as session:
            yield session

    previous = app.dependency_overrides.copy()
    app.dependency_overrides[get_db] = override_db
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app, raise_app_exceptions=False),
                                    base_url='http://union-take.test') as client:
            response = await client.post('/api/trades/', json={
                'order_id': str(order_id), 'quantity_mt': '200', 'expected_terms_digest': digest,
            }, headers={'Authorization': f'Bearer {create_access_token(str(buyer_id))}',
                        'Idempotency-Key': f'union-{product_code}-take'})
        assert response.status_code == 200, response.text
        async with factory() as session:
            trade = (await session.execute(select(Trade))).scalar_one()
            assert trade.product_id == product.id
            assert trade.market_product == product_code
            assert trade.market_snapshot_version == 1
            assert trade.fame_terms_snapshot is None
            assert trade.lng_terms_snapshot is None
            assert trade.quantity_mt == Decimal('200')
            stored = await session.get(OrderBookOrder, order_id)
            assert stored.remaining_quantity_mt == Decimal('200')
    finally:
        app.dependency_overrides.clear()
        app.dependency_overrides.update(previous)
