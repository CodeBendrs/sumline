# Launch checklist

Last updated 2026-10-04. This is a working list, not a claim of readiness. SumLine is a development build: live Gmail/Outlook capture, the Docker image, production hosting and the legal position are all unverified. "Done" below means built and tested on the development Mac, with the evidence named.

## A. Built and tested locally

| Item | Evidence |
|---|---|
| Email verification, password reset, invitations require a verified address (when SMTP is configured) | Router-level Rust tests in `src/accounts.rs`; walked through in a browser against a local SMTP sink. No real mail provider used. |
| Public deployments refuse to start without SMTP | Startup check in `src/main.rs` (not exercised on a public host) |
| Sign-in rate limit counts real visitors behind a reverse proxy | Unit test of the forwarded-address rule; `INVOICE_TRUSTED_PROXY` |
| Consistent live backup, restore verification, pruning | `scripts/backup.sh` run against a running instance; a snapshot restored into a fresh directory and signed in to |
| `/healthz` and `--healthcheck` | Tested against a running instance; Dockerfile `HEALTHCHECK` added but the image is unbuilt |
| Template-independent invoice reading, and a **Re-read documents** action for records in review | 18 new Rust tests including synthetic layouts; the five real invoices run end to end on a scratch instance (no address -> all in review; add address, re-read -> three ready, two correctly held). Generic reader matched 50 of 50 fields on the five real layouts with the hard-coded parsers switched off, but it was tuned on those five. |
| Supplier memory, generic line items, right-typed review fields (CAD/USD dropdown, numeric amounts, supplier suggestions) | Unit and router tests; the real Jacked Factory invoice held, approved, then a second invoice from that supplier in a different layout went straight to ready while another supplier stayed held; line items matched the validated parsers on 189 of 190 fields and reached the exported workbook. Only five real layouts tried. |
| Existing behaviour unaffected | 64 Rust tests, clippy with warnings denied, and `tests/smoke.py` (without the supplied sample PDFs) pass |

## B. Needs you, in this order

Each step names what I do once you finish it. Never paste secrets into chat; put them in a local `.env` (git-ignored) and tell me it is done.

1. **Connect a real Gmail mailbox (unblocks everything else).**
   - Follow "Google setup" in `GMAIL_TESTING.md`: Cloud project, Gmail API, audience with your Gmail as a test user, the `gmail.readonly` scope, a Web client with redirect `http://localhost:3030/api/oauth/google/callback`.
   - Copy `.env.example` to `.env` and fill `GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET`.
   - Tell me. I restart the server, then you click **Connect Google account** and complete Google's own sign-in and consent (I cannot and will not enter your Google credentials).
   - Send test invoices to that Gmail and run the table in `GMAIL_TESTING.md`. I read the results and fix what fails.
2. **Give me real invoices.** Put 10 or more PDFs from your actual suppliers (a variety of suppliers, including awkward ones) in a folder outside the repository and tell me the path. I measure how many process without review and add parsers. PDFs stay out of Git.
3. **Choose hosting and a domain.** For a pilot, a small server you control plus a domain pointing at it is enough (`DEPLOYMENT.md`, `deploy/Caddyfile.example`). Tell me the choice and I prepare exact deploy files. Because Docker is not installed here, either install Docker so I can build and run the image, or choose a non-Docker setup.
4. **Choose an email-sending service and sending domain.** I need `SMTP_URL` and `MAIL_FROM` from it, and you must publish its SPF and DKIM DNS records or verification and reset mail will often be filtered as spam.
5. **Operator and legal details.** The privacy notice is a draft. Provide: the legal name of the operator, a privacy contact email, where the data is hosted, and how long data and backups are kept. I put them into the notice. Then have a lawyer review it for the places you will sell to. Invoices here show Canadian suppliers and GST/TPS; ask counsel which Canadian privacy rules apply to you. I am not giving legal advice.
6. **Decide the Gmail strategy before paying for verification.** Reading mail with `gmail.readonly` is a restricted scope: public use needs Google's verification and a security assessment, which is slow and can cost money. The alternative is a dedicated forwarding address per workspace (customers forward or auto-forward invoices to it), which needs no Google verification but changes the product. Pilot users can use the Testing audience either way. Tell me which direction you want.
7. **Google verification (only if you keep the OAuth design).** Needs the live flow working, a public domain, a hosted privacy policy and a demo video. You submit it from the Cloud Console's Verification Center; I can prepare the scope justification and test notes.
8. **Pricing and payments, if you will charge.** There is no billing code. Tell me the model (per store, per workspace, flat) and the payment provider; this is a product decision I will not guess.

## C. I do next, without waiting on you

- Account closure and data deletion for a user and for a workspace (the privacy notice already promises a process).
- Workspace-owner transfer.
- Optional two-factor sign-in (TOTP).
- A persisted continuation cursor for large mailbox backfills.
- Per-store access restrictions.
- Learn from corrections: when a person fixes a supplier or field in review, use that next time.
- Measure the reader on your real invoices (step 2) and fix whatever it gets wrong.

## D. Known risks to keep in view

- **Decided: no AI model reads invoices.** Invoice reading stays deterministic and local, so invoice contents are never sent to an outside provider. Accuracy comes from the layout reader, explicit sender labels where a supplier prints them, and learning from approvals.

- Extraction accuracy on suppliers beyond the four supplied layouts is unknown, and scanned PDFs always need manual review (no OCR). If most real invoices need review, the product's value drops regardless of the other work.
- The encryption key is a local file by default; production should keep it on separate managed storage.
- One SQLite process on one host: fine for a pilot, not for scale or high availability.
- No independent security review or penetration test has been done.
