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
    'PHASE0_SOURCE_STATE', 'PHASE0_LEGACY_VALUE', 'PHASE0_REPORT', 'PHASE0_COPY',
)


class Refusal(ValueError):
    """A refusal that may name the gate which refused.

    ``code`` is None for internal refusals whose detail stays in the private
    on-host report; the CLI then prints only its generic message.
    """

    def __init__(self, code=None):
        if code is not None and code not in CODES:
            raise ValueError('Unregistered refusal code')
        super().__init__(code)
        self.code = code


def coded_gate(error):
    """First registered code in an exception chain, else None."""
    seen = set()
    current = error
    while current is not None and id(current) not in seen:
        seen.add(id(current))
        code = getattr(current, 'code', None)
        if code in CODES:
            return code
        current = current.__cause__ or current.__context__
    return None
