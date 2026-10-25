# Supplier_offers

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Supplier_offers subsystem handles **3 routes** and touches: db.

## Routes

- `GET` `/{offer_id}` params(offer_id) → in: Database, out: SupplierOfferList [db]
  `app/routers/supplier_offers.py`
- `PUT` `/{offer_id}` params(offer_id) → out: SupplierOfferList
  `app/routers/supplier_offers.py`
- `POST` `/{offer_id}/withdraw` params(offer_id) → out: SupplierOfferList
  `app/routers/supplier_offers.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/supplier_offers.py`

---
_Back to [overview.md](./overview.md)_