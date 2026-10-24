//! Commercial summary and the market-share arithmetic it shares.
//!
//! Commercial figures are live-only actuals. They are never derived from
//! reference coverage: benchmark, seed and trade-derived reference prices carry
//! no participants, volume, GMV or revenue, so they can never reach a money
//! metric here. Realized GMV buckets PAID trades by `paid_at`; commission
//! revenue buckets PAID commissions by their payment date.
//!
//! Money is quantized to cents and stays a Decimal on the wire.

use rust_decimal::Decimal;
use serde::Serialize;

use super::marketplace::{CommissionAggregate, DecimalMetricValue, TradeAggregate, money, wire};

#[derive(Clone, Debug, Serialize)]
pub struct CommercialSummary {
    pub realized_gmv_usd: DecimalMetricValue,
    pub realized_revenue_usd: DecimalMetricValue,
    #[serde(serialize_with = "wire::decimal_option")]
    pub commission_pending_usd: Option<Decimal>,
    #[serde(serialize_with = "wire::decimal_option")]
    pub commission_invoiced_usd: Option<Decimal>,
}

/// Compose the live commercial summary.
///
/// `live_trades` is the live provenance aggregate for the requested period and
/// is `None` when no live trade row matched at all; a missing row means zero
/// realized GMV, exactly as the Python reference computes it.
pub fn summarize(
    live_trades: Option<&TradeAggregate>,
    commissions: &CommissionAggregate,
) -> CommercialSummary {
    CommercialSummary {
        realized_gmv_usd: DecimalMetricValue {
            value: Some(money(
                live_trades
                    .map(|trades| trades.gmv_current)
                    .unwrap_or(Decimal::ZERO),
            )),
            previous: Some(money(
                live_trades
                    .map(|trades| trades.gmv_previous)
                    .unwrap_or(Decimal::ZERO),
            )),
            suppressed: false,
        },
        realized_revenue_usd: DecimalMetricValue {
            value: Some(money(commissions.revenue_current)),
            previous: Some(money(commissions.revenue_previous)),
            suppressed: false,
        },
        commission_pending_usd: Some(money(commissions.pending_total)),
        commission_invoiced_usd: Some(money(commissions.invoiced_total)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aggregate(gmv_current: Decimal, gmv_previous: Decimal) -> TradeAggregate {
        TradeAggregate {
            provenance: "live".into(),
            confirmed_current_strict: 0,
            confirmed_previous_strict: 0,
            legacy_current: 0,
            legacy_previous: 0,
            volume_current_strict: Decimal::ZERO,
            volume_current_legacy: Decimal::ZERO,
            volume_previous: Decimal::ZERO,
            gmv_current,
            gmv_previous,
            missing_paid_at_current: 0,
        }
    }

    #[test]
    fn commercial_money_is_cents_and_never_reference() {
        let commissions = CommissionAggregate {
            revenue_current: Decimal::new(158000, 2),
            revenue_previous: Decimal::ZERO,
            missing_payment_date: 1,
            pending_total: Decimal::new(101250, 2),
            invoiced_total: Decimal::new(115275, 2),
        };
        let trades = aggregate(Decimal::new(31600000, 2), Decimal::ZERO);
        let summary = summarize(Some(&trades), &commissions);
        // Money keeps two fractional places, exactly like the reference.
        assert_eq!(
            summary.realized_gmv_usd.value.unwrap().to_string(),
            "316000.00"
        );
        assert_eq!(
            summary.realized_revenue_usd.value.unwrap().to_string(),
            "1580.00"
        );
        assert_eq!(
            summary.commission_pending_usd.unwrap().to_string(),
            "1012.50"
        );
    }

    #[test]
    fn a_missing_live_row_reports_zero_gmv_not_null() {
        let commissions = CommissionAggregate::default();
        let summary = summarize(None, &commissions);
        assert_eq!(summary.realized_gmv_usd.value.unwrap().to_string(), "0.00");
    }
}
