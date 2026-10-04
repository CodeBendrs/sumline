# Deploying SumLine

Status: written from the code and exercised locally only. Nothing here has been run on a public host, and the Docker image has not been built or started (Docker is not installed on the development Mac). Treat it as a starting point for a private pilot, not as a launch runbook.

## Shape of a deployment

One process, one SQLite database, one encryption key file, behind an HTTPS reverse proxy.

```
visitor -> Caddy (HTTPS, port 443) -> invoice-tracker (127.0.0.1:3030) -> data directory
                                                                 \-> SMTP provider (account email)
                                                                 \-> Google / Microsoft (mailbox read)
```

Run exactly one instance per data directory. Scaling out needs PostgreSQL and shared storage first (see ARCHITECTURE.md).

## Configuration

All settings are environment variables (or an `INVOICE_CONFIG_FILE` env file kept outside the repository). `.env.example` lists them with comments.

| Setting | Production value |
|---|---|
| `INVOICE_ORIGIN` | `https://your-host` (required; the app refuses a non-HTTPS public origin) |
| `INVOICE_BIND` | `127.0.0.1:3030` behind a proxy on the same host; `0.0.0.0:3030` only inside a container network |
| `INVOICE_TRUSTED_PROXY` | the proxy's address as the app sees it, for example `127.0.0.1` |
| `INVOICE_DATA_DIR` | a persistent, encrypted volume |
| `INVOICE_ENCRYPTION_KEY_FILE` | a 32-byte key file on a different volume or secret store than the data |
| `RESEND_API_KEY` (or `SMTP_URL`), `MAIL_FROM` | your mail provider; one is required, because a public deployment will not start without it. Prefer Resend on hosts that block outbound SMTP (Railway below Pro). Setting both is refused. |
| `GOOGLE_CLIENT_ID/SECRET`, `MICROSOFT_CLIENT_ID/SECRET` | from each provider's console, with the redirect URI `https://your-host/api/oauth/<provider>/callback` |
| `INVOICE_DEMO` | `0` (the sample login only works on loopback anyway) |

Check what the process sees, without printing any secret: `invoice-tracker --check-config`.

Account email only works well from a domain you control with SPF and DKIM published for the sending service; otherwise verification and reset messages are likely to land in spam. With Resend, verify a sending subdomain (for example `mail.sumline.example`) so the main domain's own mail reputation is not shared, add the DNS records it lists, create an API key limited to sending, then run `invoice-tracker --test-email you@example.com` (on Railway: `railway run` or the service shell) before inviting anyone.

## HTTPS

`deploy/Caddyfile.example` is a complete proxy configuration. Any proxy works if it forwards the original `Host` header and sets `X-Forwarded-For` to the real visitor as the last entry.

## Backups and restore

```sh
INVOICE_DATA_DIR=/path/to/data scripts/backup.sh /path/to/backups 14
```

- The snapshot is taken while the server runs and is verified before the script reports success.
- It contains encrypted documents plus readable account, workspace and store metadata, but not the key.
- Keep the key file in a separate location and a separate backup. Lose the key and every document is unrecoverable; leak key and snapshot together and every document is exposed.
- Copy snapshots off the machine. Schedule the script (cron, launchd or a systemd timer).
- To restore: stop the app, put the snapshot at `<data>/invoice-tracker.sqlite` (delete any `-wal`/`-shm` files beside it), restore the key, start the app, and run `invoice-tracker --verify-backup <data>/invoice-tracker.sqlite`. Rehearse this once before relying on it.

## Health

`GET /healthz` returns 200 when the database answers. `invoice-tracker --healthcheck` does the same without curl and is wired into the Dockerfile `HEALTHCHECK`. Point an uptime monitor at `/healthz` through the public URL.

## Before real customer data

Work through `LAUNCH_CHECKLIST.md`. The items it marks as needing the operator (provider verification, legal identity, hosting decisions, key management) are not things the code can settle.
