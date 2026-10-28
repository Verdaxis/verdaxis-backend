"""Explicit synthetic records for every copied table; no application seeders.

All timestamps and identifiers are fixed. These records are test evidence,
not customer data or public benchmark prices.
"""
from datetime import datetime, timezone, date
from decimal import Decimal
from uuid import UUID, uuid5, NAMESPACE_URL
import json

from sqlalchemy import text

TIME = datetime(2026, 1, 1, tzinfo=timezone.utc)
LATER = datetime(2026, 1, 15, tzinfo=timezone.utc)
BUYER = UUID('10000000-0000-4000-8000-000000000001')
SELLER = UUID('10000000-0000-4000-8000-000000000002')
PRODUCT = UUID('9510c713-6e39-5080-add3-0a7c29657b79')
POINT = UUID('73835e92-820e-584b-8280-bb61c63aa28e')


def uid(name):
    return uuid5(NAMESPACE_URL, 'https://phase0.example.invalid/' + name)


async def populate(connection, catalog):
    async def insert(table, **values):
        columns = {c['name']: c for c in catalog['tables'][table]['columns']}
        for key in ('created_at', 'updated_at'):
            if key in columns and key not in values:
                values[key] = TIME
        expressions = []
        for key, value in values.items():
            sql_type = columns[key]['type']
            if sql_type in ('json', 'jsonb'):
                values[key] = json.dumps(value, separators=(',', ':'))
                expressions.append(f'CAST(:{key} AS {sql_type})')
            elif sql_type.startswith('geography'):
                expressions.append(f'ST_GeogFromText(:{key})')
            else:
                if sql_type.startswith('numeric') and value is not None:
                    values[key] = Decimal(str(value))
                expressions.append(f':{key}')
        names = ','.join(f'"{key}"' for key in values)
        await connection.execute(text(f'INSERT INTO "{table}" ({names}) VALUES ({",".join(expressions)})'), values)

    await insert('delivery_points', id=POINT, name='Singapore', region='Asia', timezone='Asia/Singapore', is_active=True, created_at=TIME)
    await insert('products', id=uid('historical-product'), name='Synthetic retired product', fuel_type='Methanol',
                 fuel_grade='Historical', min_lot_size=17, spec_description='Preserve this source description', is_active=False)
    await insert('delivery_points', id=uid('historical-point'), name='Synthetic retired point', region='Historical',
                 timezone='Asia/Singapore', is_active=False)
    # Existing predecessor organizations were UNKNOWN. New synthetic REAL
    # parties exercise the normal INSERT snapshot guard before later changes.
    real_buyer, real_seller = uid('real-buyer'), uid('real-seller')
    for identity, role in ((real_buyer, 'FUEL_BUYER'), (real_seller, 'FUEL_SUPPLIER')):
        await insert('organizations', id=identity, name='Synthetic ' + role, type=role,
                     provenance='REAL', verification_status='APPROVED', created_at=TIME)
    buyer_user, seller_user, admin = uid('buyer-user'), uid('seller-user'), uid('admin')
    for identity, org, role, kyc in ((buyer_user, real_buyer, 'BUYER', 'REJECTED'),
                                      (seller_user, real_seller, 'SUPPLIER', 'APPROVED'),
                                      (admin, None, 'ADMIN', 'PENDING')):
        await insert('users', id=identity, email=f'{identity}@example.invalid', password_hash='synthetic-not-a-password-hash',
                     organization_id=org, role=role, status='APPROVED', email_verified=True,
                     kyc_status=kyc, kyc_organization_id=real_buyer if role == 'SUPPLIER' else org,
                     kyc_review_note='Synthetic retained review', created_at=TIME)
    await insert('organization_join_requests', id=uid('join'), user_id=buyer_user, organization_id=BUYER, status='PENDING')
    await insert('pending_registrations', id=uid('registration'), token_hash='a'*64,
                 email='pending@example.invalid', password_hash='synthetic-not-a-password-hash', expires_at=LATER)
    await insert('kyc_execution_grace', id=uid('grace'), user_id=buyer_user, organization_id=real_buyer,
                 kyc_status_at_enrollment='PENDING', enrolled_at=TIME, expires_at=LATER, created_by='kycg_20261014_prod')
    await insert('audit_logs', id=uid('notice'), user_id=buyer_user, action='kyc.grace_notice_sent',
                 resource_type='kyc_execution_grace', resource_id=str(uid('grace')), timestamp=TIME,
                 changes={'synthetic': True})
    await insert('user_status_transitions', id=uid('transition'), user_id=buyer_user, to_status='APPROVED',
                 effective_at=TIME, provenance='LIVE')
    await insert('user_login_days', id=uid('login'), user_id=buyer_user, activity_date=TIME.date(),
                 login_count=2, first_login_at=TIME, last_login_at=TIME)
    await insert('user_preferences', id=uid('preferences'), user_id=buyer_user, namespace='market_watch',
                 value={'market_products': ['B30','B100','BIO_LNG']}, updated_at=TIME)
    await insert('subscriptions', id=uid('subscription'), org_id=real_seller)
    await insert('referrals', id=uid('referral'), referrer_id=buyer_user, referred_user_id=seller_user, referral_code_used='SYNTHETIC')
    await insert('feedback_entries', id=uid('feedback'), user_id=buyer_user, message='Synthetic feedback', created_at=TIME)
    await insert('ports', id='ZZSYN', name='Synthetic port', country='ZZ', location='SRID=4326;POINT(103.8 1.3)')
    for _ in range(2):
        await insert('port_intelligence', port_id='ZZSYN')
    await insert('vessels', id=uid('vessel'), name='Synthetic vessel', imo_number='0000000', organization_id=real_buyer)
    await insert('producer_projects', id=uid('producer'), name='Synthetic producer', fuel_type='Methanol', country='ZZ')
    await insert('inventory_items', id=uid('inventory'), supplier_id=real_seller, fuel_type='Methanol',
                 current_stock_mt=100, port_id='ZZSYN')
    context = uid('context')
    await insert('market_support_contexts', id=context, actor_user_id=admin, organization_id=real_seller,
                 accountable_user_id=seller_user, support_reference='Synthetic support', started_at=TIME, expires_at=LATER)
    await insert('staff_capability_assignments', id=uid('capability'), user_id=admin,
                 capability='MARKET_SUPPORT_LISTINGS', reason='Synthetic capability', granted_by_user_id=admin,
                 granted_at=TIME, expires_at=LATER)
    authorization = uid('authorization')
    await insert('market_support_authorizations', id=authorization, organization_id=real_seller,
                 accountable_user_id=seller_user, product_id=PRODUCT, delivery_point_id=POINT,
                 availability_window='SPOT', quantity_mt=100, price_per_mt_usd=700,
                 authorization_expires_at=LATER, certification_declared=True, msds_available=True,
                 terms_digest='a'*64, evidence_reference='Synthetic authority', commercial_consent_version='synthetic-v1',
                 commercial_consent_reference='Synthetic consent', idempotency_key='synthetic-authority',
                 idempotency_request_hash='b'*64, created_by_actor_user_id=admin,
                 market_support_context_id=context, status='CONSUMED', consumed_at=TIME)
    for side, org, user in (('BID', real_buyer, buyer_user), ('ASK', real_seller, seller_user)):
        values = dict(id=uid(side), organization_id=org, owner_user_id=user, side=side, quantity_mt=100,
                      remaining_quantity_mt=100, price_per_mt_usd=700, product_id=PRODUCT, provenance='REAL',
                      delivery_point_id=POINT, availability_window='SPOT', created_at=TIME)
        if side == 'ASK':
            values.update(support_authorization_id=authorization, creation_method='MARKET_SUPPORT', created_by_actor_user_id=admin)
        await insert('orderbook_orders', **values)
    await insert('trades', id=uid('snapshot-trade'), buyer_id=real_buyer, seller_id=real_seller,
                 initiator_org_id=real_buyer, buyer_provenance='REAL', seller_provenance='REAL', initiated_by='BUYER',
                 quantity_mt=10, price_per_mt_usd=700, status='CONFIRMED', confirmed_at=TIME,
                 product_id=PRODUCT, product_name='Bio Methanol', fuel_type='Methanol', fuel_grade='Bio',
                 market_product='BIO_METHANOL', delivery_point_id=POINT, delivery_point_name='Singapore',
                 delivery_point_region='Asia', availability_window='SPOT', market_snapshot_version=1, created_at=TIME)
    for product_name, product_id in (('B30', 'c4cecebc-3d2d-5840-8021-57a9a11bc673'),
                                     ('B100', '74eb9cb5-f0e3-55f1-b82c-fe29b7c45bf5')):
        for side, organization, user in (('BID', real_buyer, buyer_user), ('ASK', real_seller, seller_user)):
            await insert('orderbook_orders', id=uid(product_name + side), organization_id=organization,
                         owner_user_id=user, side=side, quantity_mt=200, remaining_quantity_mt=200,
                         price_per_mt_usd=700, product_id=UUID(product_id), provenance='REAL',
                         delivery_point_id=POINT, availability_window='SPOT')
        await insert('trades', id=uid(product_name + '-trade'), buyer_id=real_buyer, seller_id=real_seller,
                     bid_order_id=uid(product_name + 'BID'), ask_order_id=uid(product_name + 'ASK'),
                     initiator_org_id=real_buyer, buyer_provenance='REAL', seller_provenance='REAL', initiated_by='BUYER',
                     quantity_mt=10, price_per_mt_usd=700, status='CONFIRMED', confirmed_at=TIME,
                     product_id=UUID(product_id), product_name=product_name, fuel_type='Biofuel', fuel_grade=product_name,
                     market_product=product_name, delivery_point_id=POINT, delivery_point_name='Singapore',
                     delivery_point_region='Asia', availability_window='SPOT', market_snapshot_version=1)
        await insert('market_command_results', id=uid(product_name + '-receipt'), actor_user_id=buyer_user,
                     effective_organization_id=real_buyer, operation='trade.confirm',
                     idempotency_key=product_name + '-synthetic', request_hash='d'*64,
                     resource_type='trade', resource_id=uid(product_name + '-trade'), response_status=200,
                     response_body={'product': product_name, 'status': 'CONFIRMED'})
    # Owner maintenance changes current party state; the stored version-1
    # snapshot must remain REAL and must not be reconstructed on import.
    await connection.execute(text("UPDATE organizations SET verification_status='REJECTED' WHERE id=:id"), {'id': real_buyer})
    await insert('match_suggestions', id=uid('suggestion'), bid_order_id=uid('BID'), ask_order_id=uid('ASK'),
                 score=80, recipient_org_id=real_buyer, status='DISMISSED')
    await insert('negotiations', id=uid('negotiation'), initiator_org_id=real_buyer, counterparty_org_id=real_seller,
                 product_id=PRODUCT, quantity_mt=10, current_price=700, last_actor_org_id=real_buyer, expires_at=LATER)
    await insert('negotiation_rounds', id=uid('round'), negotiation_id=uid('negotiation'), round_number=1,
                 proposer_org_id=real_buyer, proposed_price=700)
    await insert('rfqs', id=uid('rfq'), buyer_org_id=real_buyer, product_id=PRODUCT, quantity_mt=10, expires_at=LATER,
                 delivery_point_id=POINT, availability_window='SPOT')
    await insert('rfq_quotes', id=uid('quote'), rfq_id=uid('rfq'), seller_org_id=real_seller,
                 price_per_mt_usd=700, status='ACCEPTED')
    await connection.execute(text("UPDATE rfqs SET status='ACCEPTED', accepted_quote_id=:quote, trade_id=:trade WHERE id=:rfq"),
                             {'quote': uid('quote'), 'trade': uid('snapshot-trade'), 'rfq': uid('rfq')})
    await insert('market_command_results', id=uid('receipt'), actor_user_id=admin, effective_organization_id=real_seller,
                 support_context_id=context, operation='order.cancel', idempotency_key='synthetic-replay', request_hash='c'*64,
                 resource_type='order', resource_id=uid('ASK'), response_status=200,
                 response_body={'status': 'CANCELLED', '_private': 'preserve'}, created_at=TIME)
    for seq in (7, 91):
        await insert('market_event_outbox', id=uid(f'event-{seq}'), event_type='trade.confirmed', aggregate_type='trade',
                     aggregate_id=str(uid('snapshot-trade')), participant_org_ids=[str(real_buyer),str(real_seller)],
                     payload={'_private': 'preserve', 'schema_version': 1}, stream_seq=seq, dispatched_at=TIME, created_at=TIME)
    await connection.execute(text("SELECT setval('market_event_stream_seq', 150, true)"))
    await connection.execute(text("SELECT setval('orderbook_acceptance_ordinal_seq', 200, true)"))
    await insert('notifications', id=uid('notification'), recipient_id=buyer_user, type='ORDER_EXPIRY_REMINDER',
                 title='Synthetic reminder', message='Synthetic only', is_read=False, created_at=TIME,
                 data={'_email_only': True, '_email_payload': {'to': 'nobody@example.invalid'}, '_email_attempts': 2,
                       '_email_claimed_at': '2026-01-01T00:00:00Z', '_email_sent_at': None})
    # Raw JSON strings retain precision and distinguish SQL NULL from JSON null.
    for label, value in (('sql-null', None), ('json-null', 'null'), ('precision', '{"value":1234567890.12345678901234567890}')):
        await connection.execute(text("""
            INSERT INTO notifications (id, recipient_id, type, title, message, is_read, created_at, data)
            VALUES (:id, :user, 'SYSTEM', 'Synthetic codec', 'Synthetic only', false, :time, CAST(:value AS json))
        """), {'id': uid(label), 'user': buyer_user, 'time': TIME, 'value': value})
    await insert('price_alerts', id=uid('alert'), org_id=real_buyer, product_id=PRODUCT, direction='BELOW', threshold_usd=600)
    await insert('watchlists', id=uid('watchlist'), user_id=buyer_user, name='Synthetic watchlist')
    await insert('watchlist_entries', id=uid('entry'), watchlist_id=uid('watchlist'), product_id=PRODUCT)
    await insert('watchlist_targets', id=uid('target'), watchlist_id=uid('watchlist'), target_type='PIN',
                 market_product_code='BIO_METHANOL', delivery_point_id=POINT, availability_window_code='SPOT', order_id=uid('ASK'))
    await insert('watchlist_events', id=uid('watch-event'), watchlist_id=uid('watchlist'), watchlist_target_id=uid('target'), event_type='ORDER_UPDATED')
    market = dict(market_product='BIO_METHANOL', delivery_point_id=POINT, availability_window='SPOT')
    await insert('benchmarks', id=uid('benchmark'), **market, price_per_mt_usd=700)
    await insert('live_slice_benchmarks', id=uid('live-benchmark'), **market, side='ASK', benchmark_price_per_mt_usd=700, total_remaining_quantity_mt=100)
    await insert('market_signal_ingestion_runs', id=uid('signal-run'), signal_family='MARKET_INDICATION', source='synthetic', source_kind='MARKET_INDICATION')
    await insert('market_indications', id=uid('indication'), **market, side='ASK', price_per_mt_usd=700, source='synthetic', observed_at=TIME)
    await insert('fair_price_bands', id=uid('band'), **market, low_price_per_mt_usd=690, mid_price_per_mt_usd=700,
                 high_price_per_mt_usd=710, model_name='synthetic', source='synthetic', observed_at=TIME)
    await insert('physical_stems', id=uid('stem'), **market, quantity_mt=100, status='AVAILABLE', source='synthetic', stem_uid='synthetic', observed_at=TIME)
    await insert('seed_runs', id=uid('seed'), seed_name='synthetic-fixture', environment='test')
    await insert('market_row_quarantines', id=uid('quarantine'), source_table='orderbook_orders', source_id=uid('quarantined'),
                 original_row={'synthetic': True}, dependencies=[], environment='test', database_name='phase0_prod_test',
                 reason='Synthetic quarantine', operator='fixture', reference='synthetic')
    await insert('organization_market_approvals', organization_id=real_seller, previous_verification_status='PENDING',
                 reviewed_snapshot={'synthetic': True}, environment='test', database_name='phase0_prod_test',
                 reason='Synthetic approval', operator='fixture', reference='synthetic')
    await insert('news_items', id=uid('news'), title='Synthetic news', source='synthetic', source_url='https://example.invalid',
                 url='https://example.invalid/news', published_at=TIME, fetched_at=TIME)
    await insert('direct_orders', id=uid('direct-order'), fuel_type='METHANOL', quantity_mt=10, status='PENDING')
    await insert('direct_order_offers', id=uid('direct-offer'), direct_order_id=uid('direct-order'), supplier_id=real_seller,
                 price_per_mt_usd=700, is_accepted=False, created_at=TIME)
    await insert('traceability_events', id=uid('trace'), stage='synthetic')
    await insert('compliance_ledger', id=uid('ledger'))
    legacy = dict(product_id=PRODUCT, delivery_point_id=POINT, ci_score=20, certification_body='Synthetic',
                  price_per_mt_usd=700, availability_window='SPOT', delivery_window_start=TIME.date(), delivery_window_end=LATER.date())
    await insert('contracts', id=uid('contract'), **legacy, instrument_type='SPOT', buyer_org_id=real_buyer,
                 fuel_type='Methanol', lot_count=1, quantity_mt=10)
    await insert('supply_listings', id=uid('supply'), **legacy, supplier_org_id=real_seller, available_lots=1, remaining_lots=1)
    await insert('demand_profiles', id=uid('demand'), buyer_org_id=real_buyer, fuel_type='Methanol', min_ci_score=20,
                 quantity_mt=10, port_preference='Synthetic', delivery_window='SPOT')
    await insert('public_listings', id=uid('listing'), supplier_id=real_seller, region='Asia', fuel_type='Methanol',
                 fuel_grade='Bio', quantity_mt=10, price_per_mt_usd=700, availability_window='SPOT',
                 is_verdaxis_verified=False, status='ACTIVE', created_at=TIME, updated_at=TIME)
    await insert('orders', id=uid('legacy-order'), listing_id=uid('listing'), buyer_id=real_buyer,
                 status='PENDING', buyer_accepted_terms_at=TIME, commission_rate_pct='0.5', created_at=TIME)
    await insert('commissions', id=uid('commission'), match_id=uid('legacy-order'), trade_id=uid('snapshot-trade'),
                 amount_usd=20, status='PENDING', created_at=TIME, updated_at=TIME)
