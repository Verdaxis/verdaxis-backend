"""Refusal codes are a fixed safe vocabulary; unknown codes cannot be raised."""
import pytest


def test_every_code_is_exposed_and_safe():
    from scripts.phase0.refusal import CODES, Refusal

    assert len(set(CODES)) == len(CODES)
    for code in CODES:
        assert code.startswith('PHASE0_')
        assert Refusal(code).code == code
        assert str(Refusal(code)) == code


def test_unknown_code_is_rejected():
    from scripts.phase0.refusal import Refusal

    with pytest.raises(ValueError):
        Refusal('PHASE0_NOT_A_REAL_CODE')


def test_require_tool_rejects_every_cross_tool_manifest():
    from scripts.phase0.production import require_tool
    from scripts.phase0.refusal import Refusal

    for expected in ('bootstrap', 'copy', 'verify'):
        require_tool({'tool': expected}, expected)
        for claimed in ('bootstrap', 'copy', 'verify'):
            if claimed == expected:
                continue
            with pytest.raises(Refusal) as refused:
                require_tool({'tool': claimed}, expected)
            assert refused.value.code == 'PHASE0_TOOL_MISMATCH'
