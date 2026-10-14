# Read-only market and voyage MCP

Verdaxis serves three public tools at **POST `/mcp`**, outside the `/api` prefix.
The endpoint is enabled only in `development`, `test` and `staging`. **Production
is off.** This change does not deploy or activate staging.

The staging URL is `https://api-staging.verdaxis.exchange/mcp`. Transport is
streamable HTTP with stateless requests and JSON responses, using MCP SDK 2.3.0.
No authentication, stored voyage, order mutation or account lookup is provided.
Database tools open a session per call and never commit. Outputs contain no
organization, user, order or support identifiers.

## Tools

| Tool | Arguments | Result |
| --- | --- | --- |
| `list_markets` | `{}` | Five catalog products, their supported delivery points, and eight ports with region and EU flag. No database access. |
| `get_market_slice` | `market_product`, `delivery_point`, `availability_window` | Anonymous best bid/ask, counts, quantities, benchmark, demo status, observation time and evidence note. |
| `plan_voyage_fuel` | `request` containing vessel, connected legs, start date and assumptions | Leg energy/scope, costed strategies, missing inputs, price provenance and recommendation. |

All results are indicative planning evidence, not a quote or offer. Always state
each price's provenance. Demo and benchmark prices are **not executable**.
`CALLER` prices are assumptions, not Verdaxis quotes. FuelEU is an annual ship
balance; the planner calculates only this voyage's contribution.

Example slice call:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_market_slice","arguments":{"market_product":"UCOME_B100","delivery_point":"Singapore","availability_window":"2026-11"}}}
```

## Golden voyage

This test example fixes the clock at 2026-10-14 and seeds an eligible REAL UCOME
ASK in Singapore for November at USD 1,445/MT, without declared intensity or
energy-density overrides. Live market data can produce different results.

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "plan_voyage_fuel",
    "arguments": {
      "request": {
        "vessel": {
          "speed_knots": "14",
          "sea_consumption_mt_per_day": "40",
          "berth_consumption_mt_per_day": "5",
          "compatible_fuels": ["VLSFO", "UCOME_B100"]
        },
        "legs": [
          {"from_port": "Shanghai", "to_port": "Singapore", "distance_nm": "2200", "berth_days_at_destination": "1"},
          {"from_port": "Singapore", "to_port": "Rotterdam", "distance_nm": "8300", "berth_days_at_destination": "2"}
        ],
        "start_date": "2026-11-01",
        "assumptions": {
          "vlsfo_price_usd_per_mt": {"Shanghai": "650", "Singapore": "907"},
          "eua_price_eur_per_tco2": "80",
          "fueleu_pool_price_eur_per_tco2e": "109",
          "usd_per_eur": "1.10"
        }
      }
    }
  }
}
```

Response excerpt (the full `structuredContent` also contains all legs,
strategies, fuel uses, assumptions, warnings and the disclaimer):

```json
{
  "recommended_strategy_id": "CONVENTIONAL_POOL",
  "reporting_year": 2026
}
```

Golden strategy totals, in USD:

| Strategy | Total |
| --- | ---: |
| `CONVENTIONAL_PENALTY` | 1245140.55 |
| `CONVENTIONAL_POOL` | 1223044.44 |
| `UCOME_B100_MIN_BLEND` | 1236204.84 |
| `UCOME_B100_FULL` | 1628423.77 |

Numeric output fields are decimal strings. The green price carries
`source: LIVE_ORDER`, `availability_window: 2026-11`, and
`observed_at: 2026-10-14T12:00:00Z` in the fixed-clock test.

## Transport and limits

- Four concurrent tool calls per worker, shared by all three tools. Admission
  uses `acquire_nowait`; there is no admission queue. At capacity the MCP tool
  error contains `Verdaxis market tools are busy; retry shortly.`
- The SDK rejects request bodies above 4 MiB with HTTP 413.
- Up to ten connected legs, using the eight catalog ports. Speed must be above
  zero and at most 30 knots. The planner accepts start dates from 30 days before
  to 730 days after today. Other numerical bounds are published in tool schemas.
- Missing VLSFO, EUA, pool or exchange-rate assumptions leave affected costs
  unpriced; they are not invented. Partial strategies list missing inputs.
- Validation and planning failures return MCP `isError: true`. Market data
  failures contain `Market data is temporarily unavailable.` Unknown failures
  contain `Internal error`; logs record only tool name and exception type, not
  exception text or request bodies. The SDK adds a tool-name prefix to errors.
- DNS-rebinding protection remains enabled. Staging allows only the exact Host
  `api-staging.verdaxis.exchange` (no port suffix). Development/test allow
  `127.0.0.1:*`, `localhost:*` and exact `testserver`. The SDK wildcard checks a
  literal host followed by a colon; bare `localhost` is not a match. A rejected
  Host returns HTTP 421.
- Requests without an Origin header pass. **Every present Origin header is
  rejected with HTTP 403**, including empty and same-host values. SDK 2.3.0's
  empty origin allowlist rejects non-empty Origins, but its `if not origin`
  check accepts empty headers. A small ASGI guard closes that gap.
- `app/main.py` uses an exact-path Starlette ASGI `Route`, not a prefix `Mount`,
  to avoid redirecting POST `/mcp` to `/mcp/`. The MCP session manager runs around
  the existing lifespan yield, after dispatcher start and before dispatcher stop.

## Connect an agent

Configure OpenClaw's MCP client with name `verdaxis-market`, transport
**streamable HTTP**, and URL `https://api-staging.verdaxis.exchange/mcp`.
Do not send an Origin header. No bearer token is needed.

The planned command shape is:

```text
openclaw --profile <name> mcp add <server-name> <transport-and-url-options>
```

Check `openclaw --profile <name> mcp add --help` for the installed version's
transport and URL flags. This repository does not pin OpenClaw, so those flags
are not asserted here. Use `verdaxis-test` for acceptance; no OpenClaw profile
was changed by this implementation.

For a protocol-level listing (after staging is separately deployed):

```bash
curl --fail-with-body https://api-staging.verdaxis.exchange/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  --data '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'
```

A normal MCP client handles initialization and schema discovery. Ask the agent
to list markets first, then plan the golden voyage and disclose every price
source and assumption. Production activation requires a separate reviewed change.
