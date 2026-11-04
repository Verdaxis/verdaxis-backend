-- The durable markers that only Rust market writes raise once the market
-- switch has moved the market operations to Rust. Read-only, counts only:
-- no row data leaves the database. A Rust market write happened when any
-- count is above the baseline taken just before the switch.
--
-- audit_actions names every action whose only writers after the switch are
-- Rust routes: commission updates, support capability and context changes,
-- the assisted authorization (Python's writer is a switched route) and the
-- trade freeze and unfreeze.
SELECT jsonb_build_object(
    'outbox_evidence',
        (SELECT count(*) FROM market_event_outbox WHERE producer_evidence IS NOT NULL),
    'trusted_watchlist_events',
        (SELECT count(*) FROM watchlist_events WHERE producer = 'rust-market-v1'),
    'trade_freezes', (SELECT count(*) FROM trade_freezes),
    'command_receipts', (SELECT count(*) FROM market_command_results),
    'audit_actions',
        (SELECT count(*) FROM audit_logs WHERE action IN (
            'commission.updated',
            'market_support.capability_granted',
            'market_support.capability_revoked',
            'market_support.context_started',
            'market_support.context_exited',
            'market_support.context_revoked',
            'market_support.authorization_created',
            'market_support.authorization_revoked',
            'TRADE_FROZEN',
            'TRADE_UNFROZEN'
        )),
    'dismissed_suggestions',
        (SELECT count(*) FROM match_suggestions WHERE status = 'DISMISSED'),
    'support_contexts', (SELECT count(*) FROM market_support_contexts),
    'capability_assignments', (SELECT count(*) FROM staff_capability_assignments)
) AS markers
