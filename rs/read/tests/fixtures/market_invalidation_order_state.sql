CREATE FUNCTION invalidation_fixture_order_state(order_id uuid, new_status text, expiry timestamptz)
RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET status=new_status, expires_at=expiry WHERE id=order_id;
$$;
CREATE FUNCTION invalidation_fixture_link_trade(trade_id uuid, buyer uuid, seller uuid, bid uuid, ask uuid)
RETURNS void LANGUAGE sql AS $$
    UPDATE trades SET buyer_id=buyer,seller_id=seller,bid_order_id=bid,ask_order_id=ask WHERE id=trade_id;
$$;
