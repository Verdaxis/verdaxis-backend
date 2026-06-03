# Demo Activity and Signup Canary

Verdaxis uses two separate synthetic systems during the demo phase:

1. **Signup canary** — private operational monitoring that checks the public signup flow remains healthy.
2. **Demo market activity** — disclosed sample market activity that helps users understand how the platform behaves when live liquidity exists.

## Signup canary

The signup canary is not user-facing market activity. It should be run by the existing external Verdaxis monitor and alert through the same Telegram/cooldown path as uptime failures.

The canary checks:

- `POST /api/auth/register`
- `POST /api/auth/register-with-org`
- backend persistence of a new pending user and organization

Canary users and organizations must use obvious synthetic identifiers and must not be counted as real market participants.
Canary requests include the private monitor token and use `canary+...@*.canary.verdaxis.exchange`
addresses so the backend can exercise signup persistence without sending Resend verification emails.
Real user signups must always continue through the normal verification-email path.

## Demo market activity

Demo market activity is generated only from system-owned demo organizations. It must remain visibly labelled as demo/preview liquidity in user-facing surfaces.

Rules:

- Demo orders and trades are owned by deterministic demo organizations.
- Demo listings are blocked from real user trade execution.
- Demo orders do not auto-match with real orders.
- Demo trade-tape entries are labelled as demo.
- Activity is limited to the approved products, ports, and availability windows.
- Old generated demo activity is pruned so the market looks current without growing indefinitely.

The system must never use synthetic signups to imply real user growth. Signup canaries are monitoring data only; demo trades are sample market data only.
