# Compliance_api

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Compliance_api subsystem handles **4 routes** and touches: auth, db.

## Routes

- `GET` `/vessels/{vessel_id}/score` params(vessel_id) → out: ComplianceScoreResponse [auth, db]
  `app/routers/compliance_api.py`
- `GET` `/fleet` → out: ComplianceScoreResponse [auth, db]
  `app/routers/compliance_api.py`
- `POST` `/scenario` → in: ScenarioInput, out: ComplianceScoreResponse [auth, db]
  `app/routers/compliance_api.py`
- `GET` `/fuels` → out: ComplianceScoreResponse [auth, db]
  `app/routers/compliance_api.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/compliance_api.py`

---
_Back to [overview.md](./overview.md)_