//! Market and order write tests, offline and PostgreSQL, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary.

// Several test files here load the same small helper from support/ as their own
// module, as they did as separate binaries, so the test files stay unchanged.
#![allow(clippy::duplicate_mod)]

#[path = "integration_i1.rs"]
mod integration_i1;
#[path = "market_write_amend.rs"]
mod market_write_amend;
#[path = "market_write_cancel.rs"]
mod market_write_cancel;
#[path = "market_write_commission.rs"]
mod market_write_commission;
#[path = "market_write_confirm.rs"]
mod market_write_confirm;
#[path = "market_write_contexts.rs"]
mod market_write_contexts;
#[path = "market_write_create.rs"]
mod market_write_create;
#[path = "market_write_freeze.rs"]
mod market_write_freeze;
#[path = "market_write_inventory.rs"]
mod market_write_inventory;
#[path = "market_write_matching.rs"]
mod market_write_matching;
#[path = "market_write_pg_amend.rs"]
mod market_write_pg_amend;
#[path = "market_write_pg_cancel.rs"]
mod market_write_pg_cancel;
#[path = "market_write_pg_commission.rs"]
mod market_write_pg_commission;
#[path = "market_write_pg_confirm.rs"]
mod market_write_pg_confirm;
#[path = "market_write_pg_conservation.rs"]
mod market_write_pg_conservation;
#[path = "market_write_pg_contexts.rs"]
mod market_write_pg_contexts;
#[path = "market_write_pg_create.rs"]
mod market_write_pg_create;
#[path = "market_write_pg_effects.rs"]
mod market_write_pg_effects;
#[path = "market_write_pg_freeze.rs"]
mod market_write_pg_freeze;
#[path = "market_write_pg_freeze_enforcement.rs"]
mod market_write_pg_freeze_enforcement;
#[path = "market_write_pg_inventory.rs"]
mod market_write_pg_inventory;
#[path = "market_write_pg_locks.rs"]
mod market_write_pg_locks;
#[path = "market_write_pg_match_completion.rs"]
mod market_write_pg_match_completion;
#[path = "market_write_pg_matching.rs"]
mod market_write_pg_matching;
#[path = "market_write_pg_post_only.rs"]
mod market_write_pg_post_only;
#[path = "market_write_pg_producer.rs"]
mod market_write_pg_producer;
#[path = "market_write_pg_producer_visibility.rs"]
mod market_write_pg_producer_visibility;
#[path = "market_write_pg_publish.rs"]
mod market_write_pg_publish;
#[path = "market_write_pg_races.rs"]
mod market_write_pg_races;
#[path = "market_write_pg_residual.rs"]
mod market_write_pg_residual;
#[path = "market_write_pg_settlement.rs"]
mod market_write_pg_settlement;
#[path = "market_write_pg_stock.rs"]
mod market_write_pg_stock;
#[path = "market_write_pg_suggestions.rs"]
mod market_write_pg_suggestions;
#[path = "market_write_pg_support.rs"]
mod market_write_pg_support;
#[path = "market_write_pg_trade_membership.rs"]
mod market_write_pg_trade_membership;
#[path = "market_write_pg_trades.rs"]
mod market_write_pg_trades;
#[path = "market_write_producer.rs"]
mod market_write_producer;
#[path = "market_write_serving.rs"]
mod market_write_serving;
#[path = "market_write_settlement.rs"]
mod market_write_settlement;
#[path = "market_write_support.rs"]
mod market_write_support;
#[path = "market_write_trades.rs"]
mod market_write_trades;
#[path = "order_write_boundary.rs"]
mod order_write_boundary;
