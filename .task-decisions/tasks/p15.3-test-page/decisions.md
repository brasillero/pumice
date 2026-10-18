# Decisions — p15.3-test-page

### D1 · Active · `pumice serve` is reachable over Tailscale through `tailscale serve` and an explicit origin list
- date: 2026-10-18
- decided by: owner (2026-10-18; mechanism by claude, owner-delegated)
- origin: conversation
- area: api
- revisit if: Pumice gets an authentication policy for remote access

**Decision.** `pumice serve` keeps listening on `127.0.0.1` only. A new opt-in `serve.allowed_origins` lists exact extra origins (scheme, host, optional port) that the Host/Origin guard accepts besides loopback. On server-1, `tailscale serve` (tailnet-only HTTPS, never Funnel) proxies `https://server-1.tail59f332.ts.net:17570` to `http://127.0.0.1:17570`, and that origin is listed.

**Why.** The owner asked to open the test page over Tailscale. The Tailscale proxy keeps the client's Host header, which the loopback-only guard refuses, and HTTPS is needed for microphone access from another device. The same pattern already serves another test page on this host (port 17569).

**Rejected.** Binding `pumice serve` to the Tailscale address (AGENTS.md: listen on loopback only); turning the Host/Origin guard off; `tailscale funnel` (public internet).

**Consequences.** There is no authentication: every device that can reach server-1 on the tailnet can use the page and spend the AI subscription.
