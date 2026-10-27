//! Post-only assessment for assisted order entry.
//!
//! An assisted REAL order must rest without immediate execution. The assessment
//! decides, fail closed, whether the complete crossing set holds an eligible
//! and compatible row. It only reads and row-locks; the refusal it informs
//! belongs to order entry. Ordinary publication aggresses and never calls it.

use rust_decimal::Decimal;
use sqlx::PgConnection;
use uuid::Uuid;

use super::MarketWriteError;
use super::locks::{LockClass, Locked, OperationLockPlan};
use super::matching::{
    CrossingQuery, MAX_CROSSING_ORDERS_PER_MATCH, MatchBound, OrderRow, OwnerAdmission,
    OwnerRevalidation, Provenance, Side, candidate_acquisition_closed, changed, crossing_rows,
    order_is_execution_qualified, orders_execution_compatible, priority, require_parties_locked,
};

/// The outcome of one post-only assessment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostOnlyAssessment {
    pub would_cross: bool,
    pub indeterminate: bool,
    pub best_executable_price: Option<Decimal>,
}
impl PostOnlyAssessment {
    /// The crossing set could not be decided; it is treated as crossing.
    pub const fn indeterminate() -> Self {
        Self {
            would_cross: true,
            indeterminate: true,
            best_executable_price: None,
        }
    }
    /// No eligible compatible row crosses the order.
    pub const fn rests() -> Self {
        Self {
            would_cross: false,
            indeterminate: false,
            best_executable_price: None,
        }
    }
    /// The first eligible compatible row in priority order and its price.
    pub const fn crosses(price: Decimal) -> Self {
        Self {
            would_cross: true,
            indeterminate: false,
            best_executable_price: Some(price),
        }
    }
}

/// The customer organization the assisted order acts for, read under its lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostOnlyOrganization {
    pub id: Uuid,
    pub approved: bool,
}

/// Only a live, qualified REAL order of its own approved organization can be
/// assessed; anything else is indeterminate.
fn candidate_assessable(
    candidate: &OrderRow,
    organization: PostOnlyOrganization,
    bound: MatchBound<'_>,
) -> bool {
    candidate.remaining_quantity_mt > Decimal::ZERO
        && order_is_execution_qualified(candidate, bound)
        && candidate.execution_provenance() == Provenance::Real
        && organization.id == candidate.organization_id
        && organization.approved
}

/// The set the assessment previews: owned REAL rows of other approved
/// organizations that cross the candidate's price in its exact slice. The
/// display `is_crossed` flag is never consulted.
fn in_crossing_set(row: &OrderRow, candidate: &OrderRow, bound: MatchBound<'_>) -> bool {
    row.owner_user_id.is_some()
        && row.side == candidate.side.opposite()
        && row.product_id == candidate.product_id
        && row.delivery_point_id == candidate.delivery_point_id
        && row.availability_window == candidate.availability_window
        && row.organization_id != candidate.organization_id
        && row.execution_provenance() == Provenance::Real
        && row.organization_approved
        && row.live_at(bound.now())
        && match candidate.side {
            Side::Bid => row.price_per_mt_usd <= candidate.price_per_mt_usd,
            Side::Ask => row.price_per_mt_usd >= candidate.price_per_mt_usd,
        }
}

/// The pure assessment over a crossing set whose owners the port revalidated.
/// More than one hundred crossing rows is indeterminate; ineligible and
/// incompatible rows never block.
pub fn assess_post_only(
    candidate: &OrderRow,
    organization: PostOnlyOrganization,
    crossing: &[OrderRow],
    bound: MatchBound<'_>,
) -> PostOnlyAssessment {
    if !candidate_assessable(candidate, organization, bound) {
        return PostOnlyAssessment::indeterminate();
    }
    let mut rows: Vec<&OrderRow> = crossing
        .iter()
        .filter(|row| in_crossing_set(row, candidate, bound))
        .collect();
    if rows.len() > MAX_CROSSING_ORDERS_PER_MATCH {
        return PostOnlyAssessment::indeterminate();
    }
    rows.sort_by(|a, b| priority(a, b, candidate.side));
    rows.into_iter()
        .find(|row| {
            orders_execution_compatible(candidate, row, bound)
                && row.owner_admission == OwnerAdmission::Admitted
        })
        .map_or_else(PostOnlyAssessment::rests, |row| {
            PostOnlyAssessment::crosses(row.price_per_mt_usd)
        })
}

/// Borrows the sealed caller transaction; no connection or commit escapes.
#[allow(dead_code)]
pub(super) struct PostOnlyPort<'a> {
    conn: &'a mut PgConnection,
    plan: &'a OperationLockPlan,
    locked: &'a Locked,
}
#[allow(dead_code)]
impl<'a> PostOnlyPort<'a> {
    pub(super) fn new(
        conn: &'a mut PgConnection,
        plan: &'a OperationLockPlan,
        locked: &'a Locked,
    ) -> Result<Self, MarketWriteError> {
        locked.require_economic_phase()?;
        Ok(Self { conn, plan, locked })
    }

    /// Assess `candidate` for its customer `organization`, both held by the
    /// prefix. The previewed crossing rows are locked ascending UUID (this is
    /// not matching) and evaluated in price, acceptance ordinal and UUID order.
    pub(super) async fn assess(
        &mut self,
        candidate: &OrderRow,
        organization: Uuid,
        bound: MatchBound<'_>,
    ) -> Result<PostOnlyAssessment, MarketWriteError> {
        self.locked.require_economic_phase()?;
        self.locked.require_effect_slice(&candidate.slice())?;
        if candidate_acquisition_closed(self.plan, self.locked)
            || !self.locked.contains(LockClass::Organization, organization)
        {
            return Err(changed());
        }
        let approved: bool = sqlx::query_scalar(
            "SELECT verification_status::text='APPROVED' FROM organizations WHERE id=$1",
        )
        .bind(organization)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or_else(changed)?;
        let organization = PostOnlyOrganization {
            id: organization,
            approved,
        };
        if !candidate_assessable(candidate, organization, bound) {
            return Ok(PostOnlyAssessment::indeterminate());
        }
        let query = CrossingQuery::post_only(candidate);
        let preview = crossing_rows(self.conn, &query, &bound).await?;
        // An undecidable set acquires no candidate row.
        if preview.len() > MAX_CROSSING_ORDERS_PER_MATCH {
            return Ok(PostOnlyAssessment::indeterminate());
        }
        require_parties_locked(self.locked, &preview)?;
        let mut ids: Vec<Uuid> = preview.iter().map(|row| row.id).collect();
        ids.sort();
        let mut rows = crossing_rows(self.conn, &query.locking(&ids, true), &bound).await?;
        for row in &rows {
            self.locked
                .record_matching_resource(LockClass::Order, row.id)?;
        }
        let mut owners = OwnerRevalidation::default();
        for row in &mut rows {
            row.owner_admission = owners
                .revalidate(self.conn, self.locked, row, &bound)
                .await?;
        }
        Ok(assess_post_only(candidate, organization, &rows, bound))
    }
}

/// Read-only discovery for an assessment: the customer organization and the
/// previewed crossing set's parties. An undecidable set adds no party. Order
/// entry composes this into its command preview.
#[allow(dead_code)]
pub(super) async fn post_only_parties(
    conn: &mut PgConnection,
    candidate: &OrderRow,
    organization: Uuid,
    bound: MatchBound<'_>,
) -> Result<super::matching::DiscoveredParties, MarketWriteError> {
    let mut preview = crossing_rows(conn, &CrossingQuery::post_only(candidate), &bound).await?;
    if preview.len() > MAX_CROSSING_ORDERS_PER_MATCH {
        preview.clear();
    }
    super::matching::discover_parties(conn, &preview, [organization]).await
}

#[cfg(feature = "contract-tests")]
pub mod contract {
    use std::sync::Arc;

    use chrono::{DateTime, Utc};

    use super::super::locks::{MarketOperation, PartyLockPreview};
    use super::super::transaction::{
        BoxFuture, LockedMarketTransaction, MarketAttemptError, MarketCommand, MarketPreview,
    };
    use super::*;

    pub struct SealedPostOnly {
        pub assessment: PostOnlyAssessment,
        pub follow_up_error: Option<super::super::MarketErrorCode>,
        /// Order rows recorded in the lock evidence, in acquisition order.
        pub order_lock_order: Vec<Uuid>,
    }
    struct Assessment {
        candidate: OrderRow,
        organization: Uuid,
        at: DateTime<Utc>,
        kyc_gate_enabled: bool,
        follow_up: Option<bool>,
    }
    impl Assessment {
        fn bound(&self) -> MatchBound<'static> {
            MatchBound::new(self.at).with_kyc_gate(self.kyc_gate_enabled)
        }
    }
    impl MarketCommand for Assessment {
        type Output = SealedPostOnly;
        fn preview<'a>(
            &'a self,
            conn: &'a mut PgConnection,
            actor: Uuid,
        ) -> BoxFuture<'a, Result<MarketPreview, MarketAttemptError>> {
            Box::pin(async move {
                let discovered =
                    post_only_parties(conn, &self.candidate, self.organization, self.bound())
                        .await?;
                let parties = PartyLockPreview::new(
                    discovered.organizations,
                    discovered.users,
                    actor,
                    discovered.fingerprint,
                );
                let mut locks = OperationLockPlan::new(MarketOperation::Create);
                locks.parties(&parties)?.slices([self.candidate.slice()])?;
                Ok(MarketPreview { parties, locks })
            })
        }
        fn execute<'a>(
            &'a self,
            tx: &'a mut LockedMarketTransaction<'_>,
            _actor: &'a super::super::AuthenticatedActor,
        ) -> BoxFuture<'a, Result<SealedPostOnly, MarketAttemptError>> {
            Box::pin(async move {
                let assessment = tx
                    .assess_post_only(&self.candidate, self.organization, self.bound())
                    .await?;
                let follow_up_error = match self.follow_up {
                    Some(true) => tx
                        .prepare_match(&self.candidate, self.bound())
                        .await
                        .err()
                        .map(|error| error.code),
                    Some(false) => tx
                        .assess_post_only(&self.candidate, self.organization, self.bound())
                        .await
                        .err()
                        .map(|error| error.code),
                    None => None,
                };
                let order_lock_order = tx
                    .matching_trace()
                    .into_iter()
                    .filter(|(class, _)| *class == LockClass::Order)
                    .map(|(_, id)| id)
                    .collect();
                Ok(SealedPostOnly {
                    assessment,
                    follow_up_error,
                    order_lock_order,
                })
            })
        }
    }

    /// Assess an unsaved assisted order through the sealed command owner.
    pub async fn run_sealed_post_only(
        pool: &sqlx::PgPool,
        candidate: OrderRow,
        organization: Uuid,
        actor: Uuid,
        at: DateTime<Utc>,
        kyc_gate_enabled: bool,
    ) -> Result<SealedPostOnly, MarketWriteError> {
        run_assessment(
            pool,
            candidate,
            organization,
            actor,
            at,
            kyc_gate_enabled,
            None,
        )
        .await
    }
    pub async fn run_sealed_post_only_followed_by(
        pool: &sqlx::PgPool,
        candidate: OrderRow,
        organization: Uuid,
        actor: Uuid,
        at: DateTime<Utc>,
        match_next: bool,
    ) -> Result<SealedPostOnly, MarketWriteError> {
        run_assessment(
            pool,
            candidate,
            organization,
            actor,
            at,
            true,
            Some(match_next),
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn run_assessment(
        pool: &sqlx::PgPool,
        candidate: OrderRow,
        organization: Uuid,
        actor: Uuid,
        at: DateTime<Utc>,
        kyc_gate_enabled: bool,
        follow_up: Option<bool>,
    ) -> Result<SealedPostOnly, MarketWriteError> {
        super::super::transaction::run_market_command(
            pool.clone(),
            Arc::new(tokio::sync::Semaphore::new(1)),
            crate::auth::VerifiedAccess {
                subject: actor,
                issued_at: Some(Utc::now()),
                issuance_out_of_range: false,
            },
            Assessment {
                follow_up,
                candidate,
                organization,
                at,
                kyc_gate_enabled,
            },
            super::super::AdmissionClass::Cleanup,
        )
        .await
    }
}
