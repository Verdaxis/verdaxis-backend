# Task 13 native mixed integration failure classification

> **Correction (2026-10-17, branch `fix/rust-ci-mixed-caddy`).** The "locally
> minted JWT lacks issuer/audience" and "shared login rate counter (HTTP 429)"
> categories below are wrong. The suite's JWT factories already set `iss`/`aud`,
> and no login returned HTTP 429. All 48 failures and 6 errors had one cause: the
> mixed Caddy run never provisioned the `itest-*` identities the suite
> authenticates as. Corpus seeding left no such `users` or `organizations` rows
> (verified against the disposable database: 0 rows, 0 `sg-sin` ports), so logins
> returned 401 `Incorrect username or password` and every minted token named a
> non-existent user (`Could not validate credentials`). Fixed in the runner by
> seeding the identities before the suite runs
> (`scripts/rust_contract/seed_integration.py`); the table below is kept as the
> dated observation it was.

Native amd64, unchanged integration suite: 24 passed, 49 failed, 6 setup errors.
Zero timeout failures; no unclassified failure. This classifies the observed first
failure in each test, not assertions unreachable behind that failure. No Phase 0
fixture fixes were copied. Evidence: `work/rust-phase1/task13-native-mixed.log`.
JWT factories: `tests/integration/test_orderbook.py:43`, `test_trades.py:44`,
`test_marketplace.py:15`. Invalid emails/admin credentials: `tests/conftest.py:120,145`.

| Test | Observed failure category |
| --- | --- |
| `tests/integration/test_api_endpoints.py::TestAuthEndpoints::test_register_new_user` | Known fixture rot: synthetic .invalid email rejected (HTTP 422). |
| `tests/integration/test_api_endpoints.py::TestAuthEndpoints::test_login_pending_user` | Known fixture rot: registration failed; pending login receives HTTP 401. |
| `tests/integration/test_api_endpoints.py::TestAuthEndpoints::test_login_approved_admin` | Known fixture rot: dedicated itest admin credentials are not seeded by corpus; login HTTP 401. |
| `tests/integration/test_api_endpoints.py::TestProtectedEndpoints::test_me_with_valid_token` | Known fixture rot: dedicated itest admin credentials are not seeded by corpus; login HTTP 401. |
| `tests/integration/test_marketplace.py::test_inventory_publish_flow` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_marketplace.py::test_unauthorized_publish` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestCreateOrder::test_supplier_creates_ask_order` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestCreateOrder::test_buyer_creates_bid_order` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestCreateOrder::test_buyer_cannot_create_ask` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestCreateOrder::test_supplier_cannot_create_bid` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestMyOrders::test_list_my_orders` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestUpdateOrder::test_update_own_order` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestUpdateOrder::test_update_quantity_recalculates_remaining` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestUpdateOrder::test_cannot_update_other_orgs_order` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestUpdateOrder::test_update_nonexistent_order` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestCancelOrder::test_cancel_own_order` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestCancelOrder::test_cannot_cancel_other_orgs_order` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestCancelOrder::test_cancel_nonexistent_order` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_orderbook.py::TestCancelOrder::test_cannot_cancel_already_cancelled` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestCreateTrade::test_buyer_hits_ask_order` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestCreateTrade::test_seller_hits_bid_order` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestCreateTrade::test_order_remaining_quantity_decreases` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestCreateTrade::test_order_fully_filled` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestCreateTrade::test_cannot_exceed_remaining_quantity` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestCreateTrade::test_cannot_self_trade` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestCreateTrade::test_buyer_cannot_hit_bid` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestCreateTrade::test_supplier_cannot_hit_ask` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestCreateTrade::test_cannot_hit_cancelled_order` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestCreateTrade::test_hit_nonexistent_order` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestListMyTrades::test_list_trades_as_buyer` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestListMyTrades::test_list_trades_as_seller` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestListMyTrades::test_trade_response_shape` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestConfirmTrade::test_counterparty_confirms` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestConfirmTrade::test_initiator_cannot_confirm` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestConfirmTrade::test_cannot_confirm_non_pending_trade` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestDeclineTrade::test_counterparty_declines` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestDeclineTrade::test_decline_restores_order_quantity` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestDeclineTrade::test_initiator_cannot_decline` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestDeliverTrade::test_deliver_with_final_details` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestDeliverTrade::test_cannot_deliver_unconfirmed_trade` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestDeliverTrade::test_unauthorized_party_cannot_deliver` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestDeliverTrade::test_final_price_outside_band_rejected` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestDeliverTrade::test_final_price_at_band_edge_allowed` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestPayTrade::test_seller_marks_paid` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestPayTrade::test_buyer_cannot_mark_paid` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestPayTrade::test_cannot_pay_undelivered_trade` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestFullTradeLifecycle::test_full_buyer_initiated_lifecycle` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestFullTradeLifecycle::test_full_seller_initiated_lifecycle` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_trades.py::TestFullTradeLifecycle::test_partial_fills_multiple_trades` | Known fixture rot: locally minted JWT lacks issuer/audience; HTTP 401, or missing id after rejected create. |
| `tests/integration/test_auth_hardening.py::TestDualTokenLogin::test_login_returns_both_tokens` | Known fixture rot: shared login rate counter (HTTP 429). |
| `tests/integration/test_auth_hardening.py::TestDualTokenLogin::test_access_token_works_for_me` | Known fixture rot: shared login rate counter (HTTP 429). |
| `tests/integration/test_auth_hardening.py::TestTokenRefresh::test_refresh_returns_new_tokens` | Known fixture rot: shared login rate counter (HTTP 429). |
| `tests/integration/test_auth_hardening.py::TestTokenRefresh::test_access_token_as_refresh_rejected` | Known fixture rot: shared login rate counter (HTTP 429). |
| `tests/integration/test_auth_hardening.py::TestRBACEnforcement::test_buyer_cannot_access_audit_logs` | Known fixture rot: shared login rate counter (HTTP 429). |
| `tests/integration/test_trade_lifecycle.py::TestTradeLifecycle::test_full_lifecycle` | Known fixture rot: shared login rate counter (HTTP 429). |
