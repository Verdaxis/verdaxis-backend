# Caddy fallback for the Rust read groups

20 October 2026. Branch `fix/rust-caddy-fallback` from `origin/staging`
`55e5241a` (same-meaning release 1, live in production since 13:53Z).

## Problem

Production Caddy sent the four public read groups (simple, orderbook, prices,
curves) to Rust on `127.0.0.1:8010`. A stopped or restarting Rust service
returned 502 even though Python on `127.0.0.1:8000` serves the same routes. On
20 October a deploy restarted Python and produced 24 × 502 in 12 seconds
(13:53:06–13:53:18Z), 19 of them from a real user's browser.

## Change

`deploy/caddy/rust-public-staging.caddy` gained a sixth import argument: the
environment's Python upstream. Every read group now reads

```
reverse_proxy {args[N]} {args[5]} {
	lb_policy first
	lb_try_duration 10s
	lb_try_interval 250ms
	fail_duration 30s
	max_fails 1
	flush_interval -1
	transport http {
		keepalive off
	}
}
```

`lb_policy first` keeps the reviewed order: Rust, then Python. `fail_duration`
and `max_fails` enable passive health, so after one refused connection later
requests select Python directly instead of redialing Rust.

`scripts/rust_caddy_candidate.py`:

- Staging group candidates now require six import arguments, with argument 5
  exactly the environment's Python upstream.
- `production_group` emits `reverse_proxy 127.0.0.1:8010 127.0.0.1:8000`; the
  unchanged Python process is the second upstream.
- A new `--group fallback` candidate adds to the catch-all Python proxy
  `lb_try_duration 15s` and `lb_try_interval 250ms`. Production finds the proxy
  inside the reviewed final bare `handle`; staging finds the final `handle` in
  the import's enclosing `route`. It refuses an absent, ambiguous, already
  changed or non-final anchor.
- The catch-all does not get passive health. With one upstream, a remembered
  failure lets Caddy stop selecting it, so the retry would fail before the
  restart finished.
- MCP keeps a single upstream (`{args[4]}`), because it carries POST and DELETE.

Production and staging outputs, the access-log block, the rollback output and
the existing exact-match tests are unchanged except where the sixth argument or
the second production upstream required it.

## Which failures Caddy retries, and why a write is never sent twice

Source: Caddy v2.11.2, `modules/caddyhttp/reverseproxy/reverseproxy.go`
`LoadBalancing.tryAgain` (lines 1240–1303) and `httptransport.go` (lines
290–296).

1. A connection that cannot be established produces `DialError{err}`
   (`httptransport.go:295`). `tryAgain` tests `proxyErr.(DialError)` and skips
   the method check entirely, so **a dial error is retried for any method**.
   This is safe because no request bytes were sent: the comment at
   `reverseproxy.go:1256-1260` states "without a connection, no HTTP request can
   be transmitted".
2. Any other error happened after a connection succeeded. There Caddy assumes
   the upstream may have received the request. With no `retry_match`, it
   refuses to retry when `req.Method != "GET"` (`reverseproxy.go:1270-1273`).
   POST, PUT, PATCH, DELETE and the MCP POST therefore run at most once per
   upstream.
3. The same function also stops at `try_duration`, stops after `retries`, and
   waits `try_interval` between attempts. `reverseproxy.go:1269` exempts
   `errNoUpstream` from the method check, because no upstream was selected at
   all.

Consequence for this change: a POST is retried only when the connection was
refused (nothing was sent), and it is never replayed after a connection
succeeded. `tests/unit/test_rust_caddy_fallback.py` proves both halves against
real Caddy 2.11.2.

## Tests on the staging test host

Wrapper `verdaxis-staging-test "$PWD"`, every call through
`/home/verdaxis-test/bin/heavy run 3600`. Host Caddy version:
`v2.11.2 h1:iOlpsSiSKqEW+SIXrcZsZ/NO74SzB/ycqqvAIEfIm64=`.
No measurement ran (decision 110).

| Gate | Exit | Result |
| --- | --- | --- |
| Focused Caddy suite, first run | 1 | 15 passed, 3 failed: two wrong adapted-JSON paths (`try_duration` and `selection_policy` are nested under `load_balancing`), one structural negative placed a handle after the import's `route` child check. All three were test defects, corrected. |
| Focused Caddy suite, second run | 0 | 18 passed |
| Full unit suite | 1 | 2524 passed, 2 failed |

The two failures are
`tests/unit/test_assert_host_git_owner.py::test_tuple_git_commands_run_as_the_checkout_owner`
and `::test_readiness_check_keeps_owner_git_argv`, both `Invalid artifact` from
`scripts/phase0/assert_host.py`. `docs/rust-phase1/production-path.md` already
records them as a property of the staging test host's shared checkout; this lane
changed neither file. GitHub CI is the authority for that suite.

The focused suite covers:

| Test | What it proves |
| --- | --- |
| `test_real_caddy_adapts_and_validates_production_candidates` | `caddy validate` and `caddy adapt` exit 0 for P0, fallback and each cumulative production group; adapted JSON carries Rust-then-Python upstreams, `first` policy, 10 s / 250 ms, passive health, 15 s catch-all window |
| `test_real_caddy_gives_every_staging_group_the_python_fallback_transport` | the staging snippet validates and adapts; four read proxies have two upstreams, MCP and catch-all have one |
| `test_rust_serves_a_get_and_python_serves_it_when_rust_is_down` | (a) Rust up → served by Rust; Rust stopped → two consecutive GETs served by Python, no 502 |
| `test_get_waits_for_python_to_come_back` | (c) Python stopped, a GET blocks, Python restarted after 5 s, the GET succeeds and waited at least 5 s |
| `test_post_is_sent_once_after_a_refused_connection` | (d) a POST to a refused first upstream reaches Python exactly once |
| `test_post_is_not_replayed_after_the_connection_succeeded` | a POST whose connection succeeded and then failed returns 502; the second upstream receives no POST |
| `test_sse_response_streams_through_the_catch_all` | (e) the first SSE event reaches the client while the upstream still holds the second |
| Generator unit tests | exact rollback bytes, six-argument refusals, fallback insertion and refusal cases, CLI 0600 output |

## GitHub CI (draft PR #111)

| Check | Result |
| --- | --- |
| `test` (unit suite plus `tests/monitor`) | pass, 2819 passed, 7 skipped |
| `offline` | pass |
| `contract` | pass |
| `postgres-analytics` | pass |
| `postgres-phase0` | pass |
| `publish` | skipped (not a `staging`/`prod` push) |

CI confirms the two `test_assert_host_git_owner.py` failures are a property of
the staging test host, not the change: the same tests pass in CI.

## Not run

- No latency, memory or capacity measurement (decision 110).
- No live host change. Every candidate is written to a private output
  directory; the PM rolls out.

## Rollout note for the PM

The readiness requirement covers a restart of either process; the fallback
covers a stopped Rust service. Order the Caddy change before or with the
release that first needs it. `--group fallback` is independent of the four
group candidates and has its own exact rollback file.
