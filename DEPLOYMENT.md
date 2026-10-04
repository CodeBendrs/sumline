# Deploying SumLine

Status: written from the code and exercised locally only. Nothing here has been run on a public host, and the Docker image has not been built or started (Docker is not installed on the development Mac). Treat it as a starting point for a private pilot, not as a launch runbook.

## Railway, step by step (example: `sumline.tyred.ca`)

A Hobby plan is enough: account email goes over HTTPS (Resend), so Railway's SMTP block does not matter. One service, one volume, one replica; a deploy causes a short outage because a volume cannot be mounted twice.

1. **Make the secrets, on your own computer.**
   - `invoice-tracker --generate-key` prints a new `INVOICE_ENCRYPTION_KEY`. Keep a copy in a password manager: without it every stored document is unrecoverable, and it must never sit beside the database.
   - Choose an `INVOICE_SIGNUP_CODE` (any phrase) to hand to pilot users.
2. **Create the service.** Railway, New Project, Deploy from GitHub repo, choose `CodeBendrs/sumline` (allow Railway's GitHub app access to it). `railway.toml` already selects the Dockerfile, the `/healthz` check and restart policy. In the service's Settings, choose a region (US West is closest to Victoria; note it in `INVOICE_HOSTING_REGION`, because the privacy policy states it).
3. **Attach a volume** to the service with mount path `/app/data`. The container's start-up script makes it writable, then runs the app as an unprivileged user.
4. **Set variables** (service, Variables tab). Never put these in chat or in Git.

   | Variable | Value |
   |---|---|
   | `INVOICE_ORIGIN` | `https://sumline.tyred.ca` |
   | `INVOICE_ENCRYPTION_KEY` | the key from step 1 |
   | `INVOICE_SIGNUP_CODE` | the phrase from step 1 |
   | `INVOICE_TRUSTED_PROXY` | `private,100.0.0.0/8` (Railway's edge addresses, so sign-in limits count real visitors) |
   | `RESEND_API_KEY`, `MAIL_FROM` | from the Resend steps below |
   | `MAIL_REPLY_TO` (optional) | where replies to account email go; defaults to `INVOICE_PRIVACY_EMAIL`, because the sender is a no-reply address |
   | `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET` | from Google Cloud |
   | `INVOICE_OPERATOR_NAME`, `INVOICE_OPERATOR_ADDRESS`, `INVOICE_PRIVACY_EMAIL`, `INVOICE_GOVERNING_LAW` | who runs the service, as the privacy policy and terms must state it (for example `British Columbia`) |
   | `INVOICE_HOSTING_PROVIDER`, `INVOICE_HOSTING_REGION` | `Railway`, and the region you chose, for example `United States (California)` |
   | `INVOICE_BACKUP_RETENTION` | how long backups really last, for example `90 days` (see step 9) |

   In Railway's Variables tab type values as they are, with no quotes. In an env file (`.env`, `INVOICE_CONFIG_FILE`) a value with spaces, such as an operator name, must be in double quotes, and the app stops at start-up, naming the setting, if a line cannot be read. Do not set `INVOICE_BIND` or `PORT`: Railway provides `PORT` and the app listens on it. The app refuses to start on a public address if the encryption key, signup code, mail backend or operator details are missing, and says which.
5. **Add the domain.** Service, Settings, Networking, Custom Domain, enter `sumline.tyred.ca`. Railway shows a **CNAME** target and a **TXT** verification record; add both where `tyred.ca`'s DNS is managed (both are required, or the site answers 404). `tyred.ca` uses GoDaddy's nameservers, so that is GoDaddy, Domain Portfolio, `tyred.ca`, DNS, Add New Record. Its `www` and `api` records follow the same pattern, so copy it: a `CNAME` named `sumline` pointing at the `*.up.railway.app` target Railway shows, and a `TXT` named `_railway-verify.sumline` with the value Railway shows (GoDaddy wants only the part before `.tyred.ca` as the name). Leave the existing records alone. Railway then issues the certificate by itself. The `*.up.railway.app` address will answer 400 by design: the app only serves its configured host.
6. **Set up email (Resend).** `tyred.ca` is already a verified Resend domain (the Tyred site uses it), so SumLine reuses it and needs no new DNS records: set `MAIL_FROM="SumLine <no-reply@tyred.ca>"` (the existing DMARC record passes because the DKIM signature is aligned). Create a separate sending-only API key for SumLine in the Resend dashboard, so it can be revoked without touching Tyred. SumLine and Tyred then share one sending domain and its reputation; a subdomain such as `mail.sumline.tyred.ca` would separate them, at the cost of new records. Set `RESEND_API_KEY` and `MAIL_FROM="SumLine <no-reply@mail.sumline.tyred.ca>"`. Check it before inviting anyone: link the Railway CLI to the project and run `railway run ./target/release/invoice-tracker --test-email you@example.com` after a local `cargo build --release` (this runs on your computer with the project's variables and just sends one message).
7. **Google Cloud** (OAuth client): add the authorized redirect URI `https://sumline.tyred.ca/api/oauth/google/callback` (keep the localhost one). On the consent screen set the app home page `https://sumline.tyred.ca`, privacy policy `https://sumline.tyred.ca/privacy`, terms `https://sumline.tyred.ca/terms`, authorized domain `tyred.ca`, and add your pilot users as test users. See `GOOGLE_VERIFICATION.md`.
8. **First visit.** Open `https://sumline.tyred.ca`, create your account with the signup code, add your store address, connect Gmail, forward a test invoice. `https://sumline.tyred.ca/healthz` should answer `{"ok":true}`.
9. **Backups.** Turn on the volume's backup schedules in Railway (daily keeps 6 days, weekly 27, monthly 89; they cannot be exported). `INVOICE_BACKUP_RETENTION` in step 4 must match the longest schedule you actually enable. Railway backups stay inside Railway, so also take your own copy: `scripts/backup.sh` writes a verified snapshot (it needs access to the volume, so run it through `railway ssh` or a scheduled job you add), and always keep the encryption key separately.

To move to a different host later (a Raspberry Pi, a VPS), restore a snapshot into a new data directory with the same key and run `invoice-tracker --verify-backup` on it first; that path is tested.

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
| `INVOICE_TRUSTED_PROXY` | the proxy's addresses as the app sees them: addresses, ranges (`10.0.0.0/8`) or the word `private`, for example `127.0.0.1` for Caddy on the same host |
| `INVOICE_DATA_DIR` | a persistent, encrypted volume |
| `INVOICE_ENCRYPTION_KEY` (or `INVOICE_ENCRYPTION_KEY_FILE`) | base64 key from `--generate-key`, in the host's secret variables; or a 32-byte key file on a different volume than the data. Required: a public deployment never creates a key beside the data. |
| `RESEND_API_KEY` (or `SMTP_URL`), `MAIL_FROM` | your mail provider; one is required, because a public deployment will not start without it. Prefer Resend on hosts that block outbound SMTP (Railway below Pro). Setting both is refused. |
| `GOOGLE_CLIENT_ID/SECRET`, `MICROSOFT_CLIENT_ID/SECRET` | from each provider's console, with the redirect URI `https://your-host/api/oauth/<provider>/callback` |
| `INVOICE_SIGNUP_CODE` | a phrase pilot users must enter to register (invited teammates are exempt). A public deployment refuses to start without it, unless `INVOICE_ALLOW_OPEN_SIGNUP=1`. |
| `INVOICE_OPERATOR_NAME`, `_ADDRESS`, `INVOICE_PRIVACY_EMAIL`, `INVOICE_GOVERNING_LAW`, `INVOICE_HOSTING_PROVIDER`, `INVOICE_HOSTING_REGION`, `INVOICE_BACKUP_RETENTION` | fill in the privacy policy (`/privacy`) and terms (`/terms`). All but the last are required on a public deployment. Make every one true of the real service. |
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
