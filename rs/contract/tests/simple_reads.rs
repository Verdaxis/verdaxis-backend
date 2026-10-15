//! Task 8 group parity for the simple public reads (R01-R05).
//!
//! The disposable runner supplies both endpoint URLs and the token through the
//! environment, so this target is compiled only with the `contract-http`
//! feature. A plain `cargo test --workspace` run compiles the file to nothing
//! and never needs a live server, which is why the whole test carries the
//! feature gate instead of a manifest entry.
//!
//! The plan names one `simple_reads` group for R01-R05; the implemented runner
//! dispatches groups, not per-route targets, so the port, catalog, and news
//! routes are compared as that single group.

#[cfg(feature = "contract-http")]
#[test]
fn simple_reads_match_python() {
    verdaxis_contract::compare_group("simple_reads").expect("simple_reads contract group");
    let python = std::env::var("RUST_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    let status = std::process::Command::new(python)
        .args(["-c", DUPLICATE_INTELLIGENCE])
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .status()
        .expect("run populated port oracle");
    assert!(
        status.success(),
        "duplicate intelligence differs from Python"
    );
}

#[cfg(feature = "contract-http")]
const DUPLICATE_INTELLIGENCE: &str = r#"
import asyncio, os
from sqlalchemy import text
from app.seeds.safety import seed_session
from scripts.rust_contract.record import record, differences

async def fixture(remove=False):
    async with seed_session() as session:
        await session.execute(text("DELETE FROM port_intelligence WHERE port_id = 'TASK8_DUP'"))
        await session.execute(text("DELETE FROM ports WHERE id = 'TASK8_DUP'"))
        if not remove:
            await session.execute(text("INSERT INTO ports (id,name,country,location,is_active) VALUES ('TASK8_DUP','Fixture','SG',ST_SetSRID(ST_MakePoint(1.2345678901234567,0.000001),4326)::geography,true)"))
            await session.execute(text("INSERT INTO port_intelligence (id,port_id,congestion_level,methanol_price_avg,captured_at) VALUES ('10000000-0000-4000-8000-000000000001','TASK8_DUP','Low',12.50,'2026-10-15T00:00:00Z'),('10000000-0000-4000-8000-000000000002','TASK8_DUP','High',99.00,'2026-10-16T00:00:00Z')"))
        await session.commit()

asyncio.run(fixture())
try:
    cases=[dict(id='duplicate-list',method='GET',path='/api/ports'),dict(id='duplicate-detail',method='GET',path='/api/ports/TASK8_DUP')]
    token=os.environ['DISPOSABLE_TEST_TOKEN']
    expected=record(os.environ['PYTHON_CONTRACT_URL'],token,cases)
    actual=record(os.environ['RUST_CONTRACT_URL'],token,cases)
    problems=differences(expected,actual)
    assert not problems, problems
finally:
    asyncio.run(fixture(True))
"#;
