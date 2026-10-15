"""Envelope encryption uses synthetic keys only."""
import base64
import json
from uuid import uuid4

import pytest
from cryptography.exceptions import InvalidTag


def key_file(tmp_path, active="old", keys=None):
    path = tmp_path / "keys.json"
    keys = keys or {"old": b"o" * 32, "new": b"n" * 32}
    path.write_text(json.dumps({"active": active, "keys": {
        name: base64.b64encode(key).decode() for name, key in keys.items()
    }}))
    return path


def metadata():
    return dict(id=uuid4(), submission_id=uuid4(), user_id=uuid4(),
                organization_id=uuid4(), kind="IDENTITY", content_type="application/pdf")


def test_round_trip_and_rotation(tmp_path):
    from app.services.kyc_document_vault import KeyRing
    ring = KeyRing.load(key_file(tmp_path))
    row = ring.encrypt(b"%PDF-private", **metadata())
    assert row.size_bytes == 12
    assert row.key_id == "old"
    assert ring.decrypt(row) == b"%PDF-private"
    assert b"%PDF-private" not in row.ciphertext
    rotated = KeyRing.load(key_file(tmp_path, active="new"))
    assert rotated.decrypt(row) == b"%PDF-private"
    assert rotated.encrypt(b"new", **metadata()).key_id == "new"
    other = ring.encrypt(b"%PDF-private", **metadata())
    assert row.ciphertext != other.ciphertext
    assert row.wrapped_key != other.wrapped_key


@pytest.mark.parametrize("field", ["ciphertext", "data_nonce", "key_nonce", "wrapped_key"])
def test_tampering_fails(tmp_path, field):
    from app.services.kyc_document_vault import KeyRing
    ring = KeyRing.load(key_file(tmp_path))
    row = ring.encrypt(b"secret", **metadata())
    value = getattr(row, field)
    setattr(row, field, bytes([value[0] ^ 1]) + value[1:])
    with pytest.raises(InvalidTag):
        ring.decrypt(row)


@pytest.mark.parametrize("field", ["id", "submission_id", "user_id", "organization_id", "kind", "content_type", "key_id"])
def test_metadata_swap_fails(tmp_path, field):
    from app.services.kyc_document_vault import KeyRing
    ring = KeyRing.load(key_file(tmp_path))
    row = ring.encrypt(b"secret", **metadata())
    replacements = {"kind": "COMPANY_REGISTRATION", "content_type": "image/png", "key_id": "new"}
    setattr(row, field, replacements.get(field, uuid4()))
    with pytest.raises(InvalidTag):
        ring.decrypt(row)


@pytest.mark.parametrize("raw", ["", "{}", "null", "[]", '{"active":"x","keys":{}}',
    '{"active":"x","keys":{"x":"bad"}}',
    '{"active":"x","keys":{"x":"YQ=="}}',
    '{"active":"x","active":"x","keys":{}}'])
def test_invalid_key_file_rejected_without_content(tmp_path, raw):
    from app.services.kyc_document_vault import KeyFileError, KeyRing
    path = tmp_path / "invalid"
    path.write_text(raw)
    with pytest.raises(KeyFileError, match="Invalid KYC document key file") as error:
        KeyRing.load(path)
    assert str(error.value) == "Invalid KYC document key file"


def test_extra_fields_and_missing_active_rejected(tmp_path):
    from app.services.kyc_document_vault import KeyFileError, KeyRing
    path = key_file(tmp_path)
    data = json.loads(path.read_text())
    data["extra"] = "private"
    path.write_text(json.dumps(data))
    with pytest.raises(KeyFileError):
        KeyRing.load(path)
    with pytest.raises(KeyFileError):
        KeyRing.load(key_file(tmp_path, active="absent"))
    with pytest.raises(KeyFileError):
        KeyRing.load(tmp_path / "missing")
