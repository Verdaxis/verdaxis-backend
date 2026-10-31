"""One final collection assertion boundary for every bound fixture.

Expected evidence is supplied by fixtures, never captured from actual output.
Callers can use ANY only for non-semantic metadata (digest/uid/stat timestamps).
"""
import json

from reader_contract import ROOT, TRANSPORT, load

SURFACES = ('units','dropins','timers','processes','usr_local','packages','cron')


def assert_collection(case, reader, result, *, scripts, blocking, markers,
                      acquired, observations, truncations):
    case.assertEqual(set(acquired), set(SURFACES))
    case.assertEqual(result['truncations'], truncations)
    case.assertEqual(result['acquired'], acquired)
    case.assertEqual(result['dependency_coverage'], 'manual-review-required')
    case.assertEqual(result['scripts'], scripts)
    case.assertEqual(result['blocking_dependencies'], blocking)
    case.assertEqual(result['observations'], observations)
    actual_markers = {row['status'] for row in blocking
                      if row['status'].startswith(('bound-exceeded:', 'overflow:'))}
    case.assertEqual(actual_markers, markers)
    case.assertEqual(blocking, sorted(blocking, key=lambda row:
                     (reader.row_priority(row), json.dumps(row, sort_keys=True))))
    for observation in observations:
        case.assertEqual(observation['acquired'], acquired[observation['surface']])
    transport = load(TRANSPORT)
    base = json.loads((ROOT/'tests/rust_contract/phase10/host-dependencies.json').read_text())
    # Exactly the bytes the host would transmit; validation is downstream.
    wire = json.dumps(result, sort_keys=True).encode()
    document = transport.collect_hosts(base, lambda argv, data: (wire, True))
    from jsonschema import Draft202012Validator
    schema = json.loads((ROOT/'tests/rust_contract/phase10/host-inventory.schema.json').read_text())
    Draft202012Validator(schema).validate(document)
    return document


def flags(*failed):
    return {surface: surface not in failed for surface in SURFACES}


def ordered(reader, rows):
    return sorted(rows, key=lambda row: (reader.row_priority(row), json.dumps(row, sort_keys=True)))


def observation(surface, text, fields=(), *, acquired=True, unparsed=(), truncations=()):
    import hashlib
    return {'surface':surface, 'sha256':hashlib.sha256(text.encode()).hexdigest(),
            'fields':list(fields), 'unparsed':list(unparsed),
            'truncations':list(truncations), 'acquired':acquired}
