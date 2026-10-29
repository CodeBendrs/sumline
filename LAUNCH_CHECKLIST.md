# Launch checklist

Last updated 2026-10-05. This is a working list, not a claim of readiness. SumLine is a development build: live Gmail/Outlook capture, the Docker image, production hosting and the legal position are all unverified. "Done" below means built and tested on the development Mac, with the evidence named.

## A. Built and tested locally

| Item | Evidence |
|---|---|
| Email verification, password reset, invitations require a verified address (when a mail backend is configured; SMTP or Resend) | Router-level Rust tests in `src/accounts.rs`; walked through in a browser against a local SMTP sink. No real mail provider used. |
| Public deployments refuse to start without a mail backend | Startup check in `src/main.rs` (not exercised on a public host) |
| Sign-in rate limit counts real visitors behind a reverse proxy | Unit test of the forwarded-address rule; `INVOICE_TRUSTED_PROXY` |
| Consistent live backup, restore verification, pruning | `scripts/backup.sh` run against a running instance; a snapshot restored into a fresh directory and signed in to |
| `/healthz` and `--healthcheck` | Tested against a running instance; Dockerfile `HEALTHCHECK` added but the image is unbuilt |
| Template-independent invoice reading, and a **Re-read documents** action for records in review | 18 new Rust tests including synthetic layouts; the five real invoices run end to end on a scratch instance (no address -> all in review; add address, re-read -> three ready, two correctly held). Generic reader matched 50 of 50 fields on the five real layouts with the hard-coded parsers switched off, but it was tuned on those five. |
| Supplier memory, generic line items, right-typed review fields (CAD/USD dropdown, numeric amounts, supplier suggestions) | Unit and router tests; the real Jacked Factory invoice held, approved, then a second invoice from that supplier in a different layout went straight to ready while another supplier stayed held; line items matched the validated parsers on 189 of 190 fields and reached the exported workbook. Only five real layouts tried. |
| Incremental, jittered mailbox checks (Gmail history cursor, resumable scans, quiet logging, backoff) | Fake-Gmail tests that assert the exact calls: a quiet check is one history call, an arrival is history, list, one fetch, a 35-page backlog pauses and finishes, an expired cursor rescans without duplicates. Not yet run against Outlook. |
| Resend as an HTTPS mail backend (for hosts that block SMTP), and `--test-email` | Tests against a fake Resend API: correct endpoint, Bearer header and body; refusals and outages never expose the key; redirects are not followed; bad recipients rejected. Never run against the real Resend (needs your key and verified domain). |
| Production hardening: `__Host-` session cookie on HTTPS (a plain fallback is never accepted), client-IP handling behind Railway or Caddy, listening on the platform's `PORT`, container start-up that fixes a root-owned volume and then drops to an unprivileged user (`railway.toml` pins the settings) | Router tests for the cookie, unit tests for the address and port rules, and a stub test of the entrypoint (`tests/entrypoint_check.sh`). The Docker image itself is still unbuilt. |
| Encryption key from an environment variable (`--generate-key`), and a public deployment never creates a key beside the data | Unit tests for every key source, for refusing two at once, and for never echoing the key |
| Pilot signup gate (code, or an invitation to that exact address), required on a public deployment | Router tests: wrong or expired codes and invitations are refused and create nothing |
| Account deletion with password and typed confirmation, and a test that scans every table for any trace of the deleted person | Storage and router tests; SQLite `secure_delete` is on |
| Privacy policy and terms of service served as public pages (`/privacy`, `/terms`), filled from the operator's own details; consent to both is recorded with the version; a public deployment will not start until the operator is named | Text and router tests; written from published guidance (PIPEDA, Google's API Services User Data Policy) and from what the code actually does, not reviewed by a lawyer |
| Existing behaviour unaffected | 98 Rust tests, clippy with warnings denied, and `tests/smoke.py` (without the supplied sample PDFs) pass |

## B. Needs you, in this order

Each step names what I do once you finish it. Never paste secrets into chat; put them in a local `.env` (git-ignored) and tell me it is done.

1. **Connect a real Gmail mailbox (unblocks everything else).**
   - Follow "Google setup" in `GMAIL_TESTING.md`: Cloud project, Gmail API, audience with your Gmail as a test user, the `gmail.readonly` scope, a Web client with redirect `http://localhost:3030/api/oauth/google/callback`.
   - Copy `.env.example` to `.env` and fill `GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET`.
   - Tell me. I restart the server, then you click **Connect Google account** and complete Google's own sign-in and consent (I cannot and will not enter your Google credentials).
   - Send test invoices to that Gmail and run the table in `GMAIL_TESTING.md`. I read the results and fix what fails.
2. **Give me real invoices.** Put 10 or more PDFs from your actual suppliers (a variety of suppliers, including awkward ones) in a folder outside the repository and tell me the path. I measure how many process without review and add parsers. PDFs stay out of Git.
3. **Host it on Railway under `sumline.tyred.ca`.** `DEPLOYMENT.md` is an exact, ordered runbook (variables, volume, domain records, Resend, Google redirect URI, backups). You do the Railway and DNS steps; I can answer anything that goes wrong on the first deploy. Seen in the consoles (read-only, 2026-10-03): the Railway account is on the Hobby plan; `tyred.ca`'s DNS is at GoDaddy and already holds `www` and `api` CNAMEs to Railway with `_railway-verify` TXT records, so `sumline` needs the same two records; the Google OAuth client has only the localhost redirect URI.
4. **Set up Resend (chosen) for account email.** `tyred.ca` is already a verified Resend domain (you confirmed), so no new DNS records are needed: use `MAIL_FROM="SumLine <no-reply@tyred.ca>"`. Create a separate sending-only API key for SumLine in the Resend dashboard, and put `RESEND_API_KEY` and that `MAIL_FROM` in the host's variables, never in chat. Then run `sumline --test-email <your address>` and tell me the result. Resend works over HTTPS, so it avoids Railway's outbound-SMTP block on non-Pro plans.
5. **Operator details for the legal pages.** The privacy policy and terms are written and live at `/privacy` and `/terms`; they name whoever runs the service. Decided: the operators are Kunwarbir Singh Padda and Nikhil Partap Singh Dhillon (no business name), the mailing address is set in the local `.env` and Railway, not in this repository (it will be public on `/privacy` and `/terms`; a PO box would keep a home address off the page), governing law is British Columbia, and `support@tyred.ca` is the privacy and reply-to contact. These are set in the local `.env`; set them again in Railway, with `INVOICE_HOSTING_PROVIDER=Railway`, a `INVOICE_HOSTING_REGION` that names the region you really choose (US West is California), and `INVOICE_BACKUP_RETENTION` matching the backup schedules you really turn on. The app will not start on a public address until these are set, and the pages must stay true: if you change hosting, region or backups, change the variables.
6. **Decide the Gmail strategy before paying for verification.** Reading mail with `gmail.readonly` is a restricted scope: public use needs Google's verification and a security assessment, which is slow and can cost money. The alternative is a dedicated forwarding address per workspace (customers forward or auto-forward invoices to it), which needs no Google verification but changes the product. Pilot users can use the Testing audience either way. Tell me which direction you want.
7. **Google verification (only if you keep the OAuth design).** Needs the live flow working, the public domain, the policy pages (done) and a demo video. `GOOGLE_VERIFICATION.md` has the consent-screen values, a drafted scope justification and the video script; you submit it from the Cloud Console. A restricted scope also needs a yearly third-party security assessment.
8. **Pricing and payments, if you will charge.** There is no billing code. Tell me the model (per store, per workspace, flat) and the payment provider; this is a product decision I will not guess.

## C. I do next, without waiting on you

- Workspace-owner transfer.
- Optional two-factor sign-in (TOTP).
- Outlook incremental sync (per-folder delta tracking and a resumable backfill); it needs a Microsoft account to test properly.
- Per-store access restrictions.
- Off-Railway backup copies on a schedule (Railway backups cannot be exported).
- Revoke Google and Microsoft access when a connection is removed or an account is deleted (today the person is told to remove it in their provider settings).
- Learn from corrections: when a person fixes a supplier or field in review, use that next time.
- Measure the reader on your real invoices (step 2) and fix whatever it gets wrong.

## D. Known risks to keep in view

- **Decided: no AI model reads invoices.** Invoice reading stays deterministic and local, so invoice contents are never sent to an outside provider. Accuracy comes from the layout reader, explicit sender labels where a supplier prints them, and learning from approvals.

- Extraction accuracy on suppliers beyond the four supplied layouts is unknown, and scanned PDFs always need manual review (no OCR). If most real invoices need review, the product's value drops regardless of the other work.
- The legal documents are researched drafts that match the product, not advice from a lawyer, and they only stay accurate while the operator settings and the product behaviour stay true.
- One SQLite process on one host: fine for a pilot, not for scale or high availability.
- No independent security review or penetration test has been done.
