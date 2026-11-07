"""Add tenant vessels and visible/hidden orders to the SQL market matrix."""

import json
import os
from pathlib import Path
from uuid import NAMESPACE_URL, uuid5

from disposable_postgres import bind_sql, psql, rows


def fid(name):
    return str(uuid5(NAMESPACE_URL, f"rust-market-contract:{name}"))


def main():
    url = os.environ["RUST_MARKET_WRITER_URL"]
    statements = []

    def execute(sql, **values):
        statements.append(bind_sql(sql, values) + ";")

    assert rows(url, "SELECT count(*) AS count FROM vessels")[0]["count"] == 0
    orgs = [fid(f"org:{name}") for name in ("real", "real_alt", "foreign_customer")]
    for org, count in [(orgs[0], 2), (orgs[1], 3), (None, 1)]:
        for number in range(count):
            identity = fid(f"overlay:vessel:{org}:{number}")
            execute(
                "INSERT INTO vessels (id, organization_id, name, imo_number) "
                "VALUES (CAST(:id AS uuid), CAST(:org AS uuid), :name, :imo)",
                id=identity, org=org, name=f"Overlay {identity}", imo=identity,
            )
    columns = [row["column_name"] for row in rows(url,
        "SELECT column_name FROM information_schema.columns "
        "WHERE table_schema='public' AND table_name='orderbook_orders' "
        "AND column_name <> 'acceptance_ordinal' ORDER BY ordinal_position")]
    visible = [fid("order:ask_ok"), fid("overlay:visible:second")]
    clones = [
        (visible[1], {"carbon_intensity_gco2_mj": "27.3", "energy_density_mj_kg": "21.5"}),
        (fid("overlay:hidden:owner"), {"owner_user_id": f"'{fid('user:rejected')}'::uuid"}),
        (fid("overlay:hidden:expiry"), {"expires_at": "'2026-10-14T12:00:00Z'::timestamptz"}),
        (fid("overlay:hidden:policy"), {"product_id": f"'{fid('product:alias')}'::uuid"}),
        (fid("overlay:hidden:bid"), {"side": "'BID'"}),
    ]
    for identity, replacements in clones:
        replacements = {"id": f"'{identity}'::uuid", **replacements}
        projection = ", ".join(replacements.get(column, f'"{column}"') for column in columns)
        names = ", ".join(f'"{column}"' for column in columns)
        execute(
            f"INSERT INTO orderbook_orders ({names}) SELECT {projection} "
            "FROM orderbook_orders WHERE id=CAST(:source AS uuid)", source=visible[0],
        )
    psql(url, "BEGIN;\n" + "\n".join(statements) + "\nCOMMIT;")
    counts = rows(url, "SELECT organization_id::text AS organization_id, count(*) AS count FROM vessels GROUP BY organization_id")
    assert {row["organization_id"]: row["count"] for row in counts} == {orgs[0]: 2, orgs[1]: 3, None: 1}
    manifest = {
        "visible_ids": visible,
        "hidden_ids": [identity for identity, _ in clones[1:]],
        "scopes": [
            {"name": name, "role": role, "organization_id": org, "count": count}
            for name, role, org, count in [
                ("administrator", "ADMIN", None, 6),
                ("member", "BUYER", orgs[0], 2),
                ("other_member", "BUYER", orgs[1], 3),
                ("empty_member", "BUYER", orgs[2], 0),
                ("no_organization", "BUYER", None, 0),
            ]
        ],
    }
    Path(os.environ["RUST_OVERLAY_TEST_CASES"]).write_text(json.dumps(manifest, indent=2))
    print("OVERLAY_FIXTURE=PASS vessels=6 scopes=5 visible=2 hidden=4")


if __name__ == "__main__":
    main()
