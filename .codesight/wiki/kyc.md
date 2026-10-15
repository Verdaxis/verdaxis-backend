# KYC

Navigation aid. Read source before changes.

## Routes (prefix `/api/kyc`)
- POST `/submit`: validate and retain two encrypted documents with status/audit.
- GET `/status`: current organization-bound KYC status.
- GET `/admin/{user_id}/documents`: ADMIN metadata, at most 50 newest rows.
- GET `/admin/documents/{document_id}`: ADMIN download after committed view audit.
- PUT `/admin/{user_id}/approve`: human review with unchanged evidence contract.
- PUT `/admin/{user_id}/reject`: human rejection.

## Sources
- `app/routers/kyc.py`
- `app/models/kyc_document.py`
- `app/services/kyc_document_vault.py`
- `app/services/kyc_document_retention.py`
- `app/services/kyc.py` (review constant only; no provider calls)
- `alembic/versions/kycd_20261015_staging.py`

Systemd credentials supply the key ring. Missing/invalid keys disable storage,
not readiness. User/organization deletion preserves evidence through RESTRICT
and explicit retention conflicts. No external KYC provider is implemented.
