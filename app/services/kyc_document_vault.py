"""Application-level envelope encryption for retained KYC evidence."""
import base64
import json
import os
import re
from pathlib import Path
from uuid import UUID

from cryptography.hazmat.primitives.ciphers.aead import AESGCM

from app.models.kyc_document import KYCDocument

_KEY_ID = re.compile(r"[A-Za-z0-9_-]{1,64}\Z")
_MAX_KEY_FILE_BYTES = 1024 * 1024


class KeyFileError(ValueError):
    """The key file is unavailable or invalid; never include its contents."""


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate field")
        result[key] = value
    return result


def _associated_data(row: KYCDocument) -> bytes:
    return json.dumps([
        "kyc-document-v1", str(row.id), str(row.submission_id), str(row.user_id),
        str(row.organization_id), row.kind, row.content_type,
    ], separators=(",", ":")).encode("utf-8")


def _wrap_data(row: KYCDocument) -> bytes:
    return json.dumps(["kyc-key-v1", str(row.id), row.key_id], separators=(",", ":")).encode("utf-8")


class KeyRing:
    def __init__(self, active: str, keys: dict[str, bytes]):
        self.active = active
        self._keys = keys

    @classmethod
    def load(cls, path: str | Path) -> "KeyRing":
        try:
            with Path(path).open("rb") as source:
                raw = source.read(_MAX_KEY_FILE_BYTES + 1)
            if len(raw) > _MAX_KEY_FILE_BYTES:
                raise ValueError("Oversized key file")
            value = json.loads(raw, object_pairs_hook=_unique_object)
            if not isinstance(value, dict) or set(value) != {"active", "keys"}:
                raise ValueError("Invalid structure")
            active, encoded = value["active"], value["keys"]
            if not isinstance(active, str) or not isinstance(encoded, dict) or active not in encoded:
                raise ValueError("Missing active key")
            keys = {}
            for key_id, text in encoded.items():
                if not _KEY_ID.fullmatch(key_id) or not isinstance(text, str):
                    raise ValueError("Invalid key entry")
                key = base64.b64decode(text, validate=True)
                if len(key) != 32 or base64.b64encode(key).decode("ascii") != text:
                    raise ValueError("Invalid key encoding")
                keys[key_id] = key
            return cls(active, keys)
        except (OSError, ValueError, TypeError):
            raise KeyFileError("Invalid KYC document key file") from None

    def encrypt(self, plaintext: bytes, *, id: UUID, submission_id: UUID,
                user_id: UUID, organization_id: UUID, kind: str, content_type: str) -> KYCDocument:
        row = KYCDocument(
            id=id, submission_id=submission_id, user_id=user_id,
            organization_id=organization_id, kind=kind, content_type=content_type,
            size_bytes=len(plaintext), key_id=self.active,
            data_nonce=os.urandom(12), key_nonce=os.urandom(12),
        )
        data_key = AESGCM.generate_key(bit_length=256)
        row.ciphertext = AESGCM(data_key).encrypt(row.data_nonce, plaintext, _associated_data(row))
        row.wrapped_key = AESGCM(self._keys[row.key_id]).encrypt(row.key_nonce, data_key, _wrap_data(row))
        return row

    def decrypt(self, row: KYCDocument) -> bytes:
        data_key = AESGCM(self._keys[row.key_id]).decrypt(row.key_nonce, row.wrapped_key, _wrap_data(row))
        return AESGCM(data_key).decrypt(row.data_nonce, row.ciphertext, _associated_data(row))
