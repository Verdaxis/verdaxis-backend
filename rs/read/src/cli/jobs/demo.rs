//! `job demo`: disclosed demo market activity (ports
//! `scripts/run_demo_activity.py` and `app/services/demo_activity.py`).
//!
//! Work in progress (Phase 10 Task 17): the pure demo book model, then the
//! phases (`order_write::transaction::system::demo` for the market writes).
pub mod model;

use sqlx::PgConnection;
use uuid::Uuid;

use model::{DEMO_ACTIVITY_BUYER_ORG_ID, DEMO_ACTIVITY_SELLER_ORG_ID};

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
/// fields. A row that already holds them is not written (no cache bump). A
/// REAL or otherwise non-DEMO row with one of these ids is never changed
/// (the update is limited to DEMO rows, and the immutable-provenance trigger
/// refuses any change of provenance) and refuses the phase.
pub async fn provision(conn: &mut PgConnection) -> Result<bool, ProvisionError> {
    use sqlx::Connection;
    let database = |_: sqlx::Error| ProvisionError::Database;
    let mut transaction = conn.begin().await.map_err(database)?;
    let mut changed = false;
    for organization in &ACTIVITY {
        let written = sqlx::query(
            "INSERT INTO organizations \
               (id, name, domain, type, supplier_tier, verification_status, provenance) \
             VALUES ($1, $2, $3, $4, $5, 'APPROVED', 'DEMO') \
             ON CONFLICT (id) DO UPDATE SET \
               name = EXCLUDED.name, domain = EXCLUDED.domain, type = EXCLUDED.type, \
               supplier_tier = EXCLUDED.supplier_tier, \
               verification_status = EXCLUDED.verification_status \
             WHERE organizations.provenance = 'DEMO' \
               AND (organizations.name, organizations.domain, organizations.type, \
                    organizations.supplier_tier, organizations.verification_status) \
                   IS DISTINCT FROM \
                   (EXCLUDED.name, EXCLUDED.domain, EXCLUDED.type, \
                    EXCLUDED.supplier_tier, EXCLUDED.verification_status)",
        )
        .bind(organization.id)
        .bind(organization.name)
        .bind(organization.domain)
        .bind(organization.organization_type)
        .bind(organization.supplier_tier)
        .execute(&mut *transaction)
        .await
        .map_err(database)?;
        changed |= written.rows_affected() > 0;
    }
    // Whatever the upsert did or skipped, both rows must now be exact.
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
