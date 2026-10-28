# Rfq

> **Navigation aid.** Route list and file locations extracted via AST. Read the source files listed below before implementing or modifying this subsystem.

The Rfq subsystem handles **6 routes** and touches: db.

## Routes

- `GET` `/{rfq_id}` params(rfq_id) → in: Annotated, out: RFQResponse
  `app/routers/rfq.py`
- `POST` `/{rfq_id}/quote` params(rfq_id) → out: RFQResponse
  `app/routers/rfq.py`
- `PUT` `/{rfq_id}/quotes/{quote_id}` params(rfq_id, quote_id) → out: RFQResponse
  `app/routers/rfq.py`
- `POST` `/{rfq_id}/quotes/{quote_id}/withdraw` params(rfq_id, quote_id) → out: RFQResponse
  `app/routers/rfq.py`
- `POST` `/{rfq_id}/accept/{quote_id}` params(rfq_id, quote_id) → out: RFQResponse
  `app/routers/rfq.py`
- `POST` `/{rfq_id}/cancel` params(rfq_id) → out: RFQResponse [db]
  `app/routers/rfq.py`

## Source Files

Read these before implementing or modifying this subsystem:
- `app/routers/rfq.py`

---
_Back to [overview.md](./overview.md)_