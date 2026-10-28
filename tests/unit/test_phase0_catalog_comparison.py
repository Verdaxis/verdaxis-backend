"""Frozen Phase 0 catalog comparison proofs against the real 2026-10-16 evidence.

The two committed fixtures are the PM's read-only `inspect_catalog` observations
of the live production source database and of the scratch copy restored with
`pg_restore` from the newest live dump. They carry schema only; the PM removed
`reference_rows` when making the evidence. Live is `en_US.UTF-8`, so its
extension members are listed in collation order, and the live database holds
both text forms of the same varchar-array expressions because it was loaded
from a dump on 1 October and later migrations added fresh-DDL objects.
"""
import copy
import json
from pathlib import Path

import pytest

from scripts.phase0.manifest import (
    canonical_expression_text,
    load_manifest,
    require_complete_inventory,
)

FIXTURES = Path(__file__).resolve().parents[1] / "phase0" / "fixtures"
LIVE = json.loads((FIXTURES / "live-source-catalog-20261016.json").read_text())
RESTORED = json.loads((FIXTURES / "restored-source-catalog-20261016.json").read_text())


def load_evidence(name):
    return copy.deepcopy(LIVE if name == "live" else RESTORED)


@pytest.mark.parametrize("name", ["live", "restored"])
def test_real_source_catalogs_pass_the_declared_inventory(name):
    require_complete_inventory(load_evidence(name), load_manifest()["source_catalog"])


def test_both_real_catalogs_are_equal_under_the_comparison():
    # The live source and the pg_restore of its newest dump are one schema.
    require_complete_inventory(load_evidence("live"), load_evidence("restored"))


def _column(catalog, table, name):
    return next(row for row in catalog["tables"][table]["columns"] if row["name"] == name)


def _constraint(catalog, table, name):
    return next(row for row in catalog["tables"][table]["constraints"] if row["name"] == name)


@pytest.mark.parametrize(
    "table,constraint",
    [
        ("user_browsing_events", "ck_user_browsing_events_action"),
        ("trades", "ck_trades_domain"),
        ("rfqs", "ck_rfqs_domain"),
        ("rfq_quotes", "ck_rfq_quotes_domain"),
    ],
)
def test_evidence_holds_both_dump_and_fresh_text_forms(table, constraint):
    # Live was loaded from a dump and later migrated: it holds form B (dump)
    # and form A (fresh DDL) of the same expressions. The restore is form B.
    live = _constraint(load_evidence("live"), table, constraint)["definition"]
    restored = _constraint(load_evidence("restored"), table, constraint)["definition"]
    assert "::character varying::text" in live or "]::text[]" in live
    assert "::character varying::text" in restored


@pytest.mark.parametrize(
    "mutation,label",
    [
        (
            lambda c: c["tables"]["users"]["columns"].append(
                {"comment": None, "default": None, "generated": "\x00", "identity": "\x00",
                 "name": "unreviewed", "not_null": False, "type": "text"}),
            "extra column",
        ),
        (
            lambda c: c["tables"]["users"].__setitem__(
                "columns", [x for x in c["tables"]["users"]["columns"] if x["name"] != "email"]),
            "missing column",
        ),
        (
            lambda c: _column(c, "users", "email").__setitem__("type", "character varying(199)"),
            "changed type",
        ),
        (
            lambda c: _constraint(c, "trades", "ck_trades_domain").__setitem__(
                "definition",
                _constraint(c, "trades", "ck_trades_domain")["definition"].replace("'PAID'", "'UNPAID'")),
            "changed CHECK literal",
        ),
        (
            lambda c: _constraint(c, "user_browsing_events", "ck_user_browsing_events_action").__setitem__(
                "definition",
                _constraint(c, "user_browsing_events", "ck_user_browsing_events_action")["definition"]
                .replace(", 'market_filter'", "")),
            "missing array element",
        ),
    ],
)
@pytest.mark.parametrize("name", ["live", "restored"])
def test_changed_source_schema_still_refuses(mutation, label, name):
    evidence = load_evidence(name)
    mutation(evidence)
    with pytest.raises(ValueError, match="Unclassified source schema"):
        require_complete_inventory(evidence, load_manifest()["source_catalog"])


@pytest.mark.parametrize(
    "mutation,label",
    [
        (lambda c: _column(c, "rfqs", "reference_number").__setitem__("type", "character varying(21)"),
         "changed legacy type"),
        (lambda c: c["tables"]["rfqs"].__setitem__(
            "indexes", [x for x in c["tables"]["rfqs"]["indexes"] if x["name"] != "ix_rfqs_reference_number"]),
         "missing legacy index"),
        (lambda c: c["tables"]["rfqs"]["constraints"].pop(), "missing legacy constraint"),
    ],
)
def test_legacy_objects_are_classified_not_ignored(mutation, label):
    evidence = load_evidence("live")
    mutation(evidence)
    with pytest.raises(ValueError, match="Unclassified source schema"):
        require_complete_inventory(evidence, load_manifest()["source_catalog"])


def test_comparison_is_independent_of_column_and_member_order():
    evidence = load_evidence("live")
    columns = evidence["tables"]["rfqs"]["columns"]
    # Move the legacy columns to the end, as a fresh build would list them.
    columns.append(columns.pop(next(i for i, x in enumerate(columns) if x["name"] == "reference_number")))
    columns.append(columns.pop(next(i for i, x in enumerate(columns) if x["name"] == "daily_seq")))
    evidence["extension_members"] = list(reversed(evidence["extension_members"]))
    require_complete_inventory(evidence, load_manifest()["source_catalog"])


CONSTRAINT_FORM_A = ("CHECK (side::text = ANY (ARRAY['BID'::character varying, 'ASK'::character varying,"
                     " 'MID'::character varying]::text[]))")
CONSTRAINT_FORM_B = ("CHECK (side::text = ANY (ARRAY['BID'::character varying::text, "
                     "'ASK'::character varying::text, 'MID'::character varying::text]))")
INDEX_FORM_A = ("CREATE UNIQUE INDEX uq ON public.negotiations USING btree (x) "
                "WHERE ((status)::text = ANY ((ARRAY['OPEN'::character varying, "
                "'COUNTERED'::character varying])::text[]))")
INDEX_FORM_B = ("CREATE UNIQUE INDEX uq ON public.negotiations USING btree (x) "
                "WHERE ((status)::text = ANY (ARRAY[('OPEN'::character varying)::text, "
                "('COUNTERED'::character varying)::text]))")


def test_constraint_dump_and_fresh_forms_compare_equal():
    assert canonical_expression_text(CONSTRAINT_FORM_A) == canonical_expression_text(CONSTRAINT_FORM_B)


def test_index_dump_and_fresh_forms_compare_equal():
    assert canonical_expression_text(INDEX_FORM_A) == canonical_expression_text(INDEX_FORM_B)


def test_dump_form_of_all_comparison_is_normalized_too():
    form_a = "CHECK (p::text <> 'DEMO'::text OR (s::text <> ALL (ARRAY['OPEN'::character varying]::text[])))"
    form_b = "CHECK (p::text <> 'DEMO'::text OR (s::text <> ALL (ARRAY['OPEN'::character varying::text])))"
    assert canonical_expression_text(form_a) == canonical_expression_text(form_b)


@pytest.mark.parametrize(
    "text,reason",
    [
        ("CHECK (buyer_id = ANY (ARRAY['4da7b285-34ee-5443-9406-f96b4ed1a251'::uuid]))", "uuid array"),
        ("CHECK (qty > 0::numeric AND price <> 'NaN'::numeric)", "numeric literals"),
        ("CHECK (status::text = 'OPEN'::text)", "single varchar literal without an array"),
        ("CHECK (side = ANY (ARRAY['BID'::bpchar]))", "non-varchar array type"),
    ],
)
def test_unrelated_text_is_returned_unchanged(text, reason):
    assert canonical_expression_text(text) == text


@pytest.mark.parametrize("value", [None, 17, True, ["a"], {"a": 1}])
def test_non_string_values_are_returned_unchanged(value):
    assert canonical_expression_text(value) is value or canonical_expression_text(value) == value


def test_a_changed_literal_inside_the_normalized_forms_still_differs():
    other = CONSTRAINT_FORM_B.replace("'BID'", "'SELL'")
    assert canonical_expression_text(CONSTRAINT_FORM_A) != canonical_expression_text(other)


def test_a_missing_element_inside_the_normalized_forms_still_differs():
    other = CONSTRAINT_FORM_B.replace(", 'MID'::character varying::text", "")
    assert canonical_expression_text(CONSTRAINT_FORM_A) != canonical_expression_text(other)
