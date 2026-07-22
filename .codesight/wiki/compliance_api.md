# Compliance_api

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Compliance_api subsystem handles **5 routes** and touches: auth, db.

## Routes

- `GET` `/api/vessels/{vessel_id}/score` params(vessel_id) → out: ComplianceScoreResponse [auth, db]
  `app/routers/compliance_api.py`
- `GET` `/api/fleet` → out: ComplianceScoreResponse [auth, db]
  `app/routers/compliance_api.py`
- `POST` `/api/scenario` → in: ScenarioInput, out: ComplianceScoreResponse [auth, db]
  `app/routers/compliance_api.py`
- `POST` `/api/pricing-overlay` → in: ScenarioInput, out: ComplianceScoreResponse [auth, db]
  `app/routers/compliance_api.py`
- `GET` `/api/fuels` → out: ComplianceScoreResponse [auth, db]
  `app/routers/compliance_api.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/compliance_api.py`

---
_Back to [overview.md](./overview.md)_