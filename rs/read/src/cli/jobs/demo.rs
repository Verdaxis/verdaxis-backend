//! `job demo`: disclosed demo market activity (ports
//! `scripts/run_demo_activity.py` and `app/services/demo_activity.py`).
//!
//! A run is Python's launcher order, each phase its own transaction:
//! provisioning (here: it writes organizations, not the order book), then the
//! market phases behind the order-write boundary (coverage per product,
//! legacy expiry, prune). The first failure stops the run; the next run
//! converges.
pub mod model;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::cli::context::{self, Purpose};
use crate::cli::gate::{require_apply, require_unit_deploy_state_absent};
use crate::cli::{Exit, JobArgs, JobKind};
use crate::order_write::{DemoMarketFailure, DemoMarketReport, run_demo_market_phases};
use model::{
    Catalog, DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID, DEMO_SEED_BUYERS,
    DEMO_SEED_SUPPLIERS,
};

/// One activity organization as provisioning keeps it (Python's
/// `ensure_demo_activity_organizations`).
struct ActivityOrganization {
    id: Uuid,
    name: &'static str,
    domain: &'static str,
    organization_type: &'static str,
    supplier_tier: Option<&'static str>,
}

const ACTIVITY: [ActivityOrganization; 2] = [
    ActivityOrganization {
        id: DEMO_ACTIVITY_BUYER_ORG_ID,
        name: "Verdaxis Demo Buyer Activity",
        domain: "demo-buyer.verdaxis.local",
        organization_type: "FUEL_BUYER",
        supplier_tier: None,
    },
    ActivityOrganization {
        id: DEMO_ACTIVITY_SELLER_ORG_ID,
        name: "Verdaxis Demo Supplier Activity",
        domain: "demo-supplier.verdaxis.local",
        organization_type: "FUEL_SUPPLIER",
        supplier_tier: Some("REGIONAL_SUPPLIER"),
    },
];

/// Why provisioning refused. Nothing it wrote is committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionError {
    /// A row with an activity organization id exists but is not DEMO, or
    /// does not hold the exact fields after the upsert.
    NotDemo,
    Database,
}

/// Create the two activity organizations, or bring them back to their exact
/// fields (Python's `merge`). It uses only the app role's grants: a new row
/// is inserted without `verification_status` (the role has no INSERT on it;
/// the row starts PENDING), then one UPDATE sets every field that differs,
/// approval included. A row that already holds them is not written (no
/// cache bump). A REAL or otherwise non-DEMO row with one of these ids is
/// never changed (the update is limited to DEMO rows, and the
/// immutable-provenance trigger refuses any change of provenance) and
/// refuses the phase.
pub async fn provision(conn: &mut PgConnection) -> Result<bool, ProvisionError> {
    use sqlx::Connection;
    let database = |_: sqlx::Error| ProvisionError::Database;
    let mut transaction = conn.begin().await.map_err(database)?;
    let mut changed = false;
    for organization in &ACTIVITY {
        let inserted = sqlx::query(
            "INSERT INTO organizations (id, name, domain, type, supplier_tier, provenance) \
             VALUES ($1, $2, $3, $4, $5, 'DEMO') \
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(organization.id)
        .bind(organization.name)
        .bind(organization.domain)
        .bind(organization.organization_type)
        .bind(organization.supplier_tier)
        .execute(&mut *transaction)
        .await
        .map_err(database)?;
        let updated = sqlx::query(
            "UPDATE organizations SET \
               name = $2, domain = $3, type = $4, supplier_tier = $5, \
               verification_status = 'APPROVED' \
             WHERE id = $1 AND provenance = 'DEMO' \
               AND (name, domain, type, supplier_tier, verification_status) \
                   IS DISTINCT FROM ($2, $3, $4, $5, 'APPROVED')",
        )
        .bind(organization.id)
        .bind(organization.name)
        .bind(organization.domain)
        .bind(organization.organization_type)
        .bind(organization.supplier_tier)
        .execute(&mut *transaction)
        .await
        .map_err(database)?;
        changed |= inserted.rows_affected() + updated.rows_affected() > 0;
    }
    // Whatever the writes did or skipped, both rows must now be exact.
    let exact: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM organizations AS o \
         JOIN (VALUES ($1::uuid, $2::text, $3::text, $4::text, $5::text), \
                      ($6::uuid, $7::text, $8::text, $9::text, $10::text)) \
              AS want(id, name, domain, type, supplier_tier) ON want.id = o.id \
         WHERE o.provenance = 'DEMO' AND o.verification_status = 'APPROVED' \
           AND o.name = want.name AND o.domain = want.domain AND o.type::text = want.type \
           AND o.supplier_tier IS NOT DISTINCT FROM want.supplier_tier",
    )
    .bind(ACTIVITY[0].id)
    .bind(ACTIVITY[0].name)
    .bind(ACTIVITY[0].domain)
    .bind(ACTIVITY[0].organization_type)
    .bind(ACTIVITY[0].supplier_tier)
    .bind(ACTIVITY[1].id)
    .bind(ACTIVITY[1].name)
    .bind(ACTIVITY[1].domain)
    .bind(ACTIVITY[1].organization_type)
    .bind(ACTIVITY[1].supplier_tier)
    .fetch_one(&mut *transaction)
    .await
    .map_err(database)?;
    if exact != 2 {
        return Err(ProvisionError::NotDemo);
    }
    transaction.commit().await.map_err(database)?;
    Ok(changed)
}

/// The phase that stopped a run.
#[derive(Debug)]
pub enum DemoFailure {
    Provision(ProvisionError),
    Market(DemoMarketFailure),
}

impl DemoFailure {
    pub fn phase(&self) -> &'static str {
        match self {
            Self::Provision(_) => "provision",
            Self::Market(failure) => failure.phase(),
        }
    }
}

/// What one run did, by count (Python's printed result).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DemoReport {
    pub organizations_written: bool,
    pub market: DemoMarketReport,
}

impl DemoReport {
    fn counts_json(&self) -> Value {
        json!({
            "organizations_written": self.organizations_written,
            "coverage_products": self.market.coverage_products,
            "coverage_created": self.market.coverage.created,
            "coverage_refreshed": self.market.coverage.refreshed,
            "coverage_expired": self.market.coverage.expired,
            "legacy_activity_expired": self.market.legacy_expired,
            "trades_pruned": self.market.prune.trades,
            "orders_pruned": self.market.prune.orders,
        })
    }
}

/// One run at `now` over the given catalog. `report` holds what committed
/// before a failure.
pub async fn run(
    pool: &PgPool,
    now: DateTime<Utc>,
    catalog: &dyn Catalog,
    report: &mut DemoReport,
) -> Result<(), DemoFailure> {
    let wanted = model::build_coverage(now, catalog, &DEMO_SEED_BUYERS, &DEMO_SEED_SUPPLIERS);
    run_with_coverage(pool, now, wanted, report).await
}

/// [`run`] with the wanted coverage given: tests limit the book to the
/// slices their fixture holds, as the Python driver does.
pub async fn run_with_coverage(
    pool: &PgPool,
    now: DateTime<Utc>,
    wanted: Vec<model::CoverageOrder>,
    report: &mut DemoReport,
) -> Result<(), DemoFailure> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|_| DemoFailure::Provision(ProvisionError::Database))?;
    report.organizations_written = provision(&mut conn).await.map_err(DemoFailure::Provision)?;
    drop(conn);
    run_demo_market_phases(pool, now, wanted, &mut report.market)
        .await
        .map_err(DemoFailure::Market)
}

/// The committed market policy as the demo book's catalog. It exists only
/// when every product and port of the demo book is in the policy, so a
/// lookup never misses.
pub struct PolicyCatalog<'a>(&'a crate::MarketPolicy);

impl<'a> PolicyCatalog<'a> {
    pub fn covering(policy: &'a crate::MarketPolicy) -> Option<Self> {
        let book = model::demo_book();
        let products = book.products.iter().all(|product| {
            policy
                .products
                .iter()
                .any(|known| known.name == product.name)
                && product
                    .ports
                    .iter()
                    .all(|port| policy.points.iter().any(|known| known.name == port.name))
        });
        products.then_some(Self(policy))
    }
}

impl Catalog for PolicyCatalog<'_> {
    fn product_id(&self, name: &str) -> Uuid {
        self.0
            .products
            .iter()
            .find(|product| product.name == name)
            .map_or(Uuid::nil(), |product| product.id)
    }
    fn delivery_point_id(&self, name: &str) -> Uuid {
        self.0
            .points
            .iter()
            .find(|point| point.name == name)
            .map_or(Uuid::nil(), |point| point.id)
    }
}

/// `job demo --environment --release-sha --apply`: the gates every writing
/// job has (apply, no deploy in progress, the purpose-bound context with its
/// identity and target checks), then one [`run`].
pub async fn command(args: &JobArgs) -> Exit {
    if let Err(exit) = require_apply(args.apply) {
        report("usage_refused", None);
        return exit;
    }
    if let Err(exit) = require_unit_deploy_state_absent(args.identity.environment) {
        report("deploy_state_refused", None);
        return exit;
    }
    let context = match context::open(
        Purpose::Job(JobKind::Demo),
        args.identity.environment,
        &args.identity.release_sha,
    )
    .await
    {
        Ok(context) => context,
        Err(exit) => {
            report("context_refused", None);
            return exit;
        }
    };
    let Ok(policy) = crate::runtime::policy::committed_policy() else {
        report("catalog_refused", None);
        return Exit::Precondition;
    };
    let Some(catalog) = PolicyCatalog::covering(&policy) else {
        report("catalog_refused", None);
        return Exit::Precondition;
    };
    let mut outcome = DemoReport::default();
    match run(&context.pool, Utc::now(), &catalog, &mut outcome).await {
        Ok(()) => {
            report("completed", Some(outcome.counts_json()));
            Exit::Ok
        }
        Err(failure) => {
            let mut counts = outcome.counts_json();
            counts["failed_phase"] = json!(failure.phase());
            report("failed", Some(counts));
            Exit::Dependency
        }
    }
}

/// The one JSON line the command prints on stderr: counts only.
fn report(outcome: &str, counts: Option<Value>) {
    eprintln!(
        "{}",
        json!({"command": "job demo", "outcome": outcome, "counts": counts.unwrap_or(json!({}))})
    );
}
