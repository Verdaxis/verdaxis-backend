"""Safe refusal reason codes for the Phase 0 production tools.

A code names the gate that refused. It never contains credentials, SQL values,
host names, file contents or paths, so it is safe on stderr and in test output.
"""
from __future__ import annotations

CODES = (
    'PHASE0_TOOL_MISMATCH', 'PHASE0_ENVIRONMENT', 'PHASE0_MANIFEST',
    'PHASE0_AUTHORITY', 'PHASE0_INSTALLED_BYTES', 'PHASE0_DB_PAIR',
    'PHASE0_RECEIPT', 'PHASE0_REPORT_PATH', 'PHASE0_SECRET',
    'PHASE0_DB_IDENTITY', 'PHASE0_SOURCE_SESSIONS', 'PHASE0_TARGET_FENCE',
    'PHASE0_SOURCE_STATE', 'PHASE0_REPORT', 'PHASE0_COPY',
)


class Refusal(ValueError):
    def __init__(self, code):
        if code not in CODES:
            raise ValueError('Unregistered refusal code')
        super().__init__(code)
        self.code = code
