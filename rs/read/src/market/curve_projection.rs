//! Canonical curve arithmetic and ordered public projections.
use rust_decimal::{Decimal, RoundingStrategy};

pub fn midpoint(bid: Option<Decimal>, ask: Option<Decimal>) -> Option<Decimal> {
    bid.zip(ask).map(|(bid, ask)| {
        ((bid + ask) / Decimal::from(2))
            .round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven)
    })
}
