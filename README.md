# SumLine

A Rust web application for capturing supplier invoices from existing Gmail or Outlook mailboxes, routing individual invoices to stores, reviewing uncertain results, and maintaining downloadable Excel workbooks and an original-document archive.

This is a working development build, not a launched customer service. Email adapters are implemented but require the operator's OAuth credentials and real-account validation. The preview privacy notice describes the current code and is explicitly a draft pending the operator's legal identity, contact, hosting, and retention decisions.

## Run

Requirements: Rust 1.97 or later. Install Poppler (`brew install poppler` on Mac, `apt-get install poppler-utils` on Linux). Poppler is the preferred PDF reader because one supplied invoice has a malformed cross-reference table that the Rust-only fallback cannot read.

```sh
cp .env.example .env
cargo run --locked --bin sumline
```

Open http://localhost:3030. Create an account and a first store. In Stores, add a specific address or store identifier from its invoices. Add more stores as needed. Upload PDFs or configure and connect email. Review uncertain records (after adding a store address or updating SumLine, **Re-read documents** on the Needs review page reads them again), then export a workbook for the selected supplier, store and date range. Original PDFs are available from each invoice or the Document archive.

On this Mac, `Launch.command` starts the supplied native binary when present, or builds the source with Cargo. It defaults to storing application data under `~/Library/Application Support/InvoiceTracker`, keeping runtime data separate from the source folder on Gold. The browser opens at the URL printed in the terminal; the launcher does not connect a mailbox or enable the sample login. The app maintains its own XLSX workbook; Google Sheets/Excel Online synchronization is not implemented.

Optional sample preview: set `INVOICE_DEMO=1` and `INVOICE_SAMPLE_DIR` to your local directory containing the four supplied PDFs. The sample workspace is only allowed when the server binds to loopback. Sample PDFs, databases, encryption keys, and OAuth secrets are excluded from Git.

## Existing email integration

The operator registers applications once; each store owner connects their existing mailbox through the provider's consent screen. Suppliers do not change email addresses or contact details.

- Google: enable Gmail API, create a web OAuth client, and register `http://localhost:3030/api/oauth/google/callback` (or your HTTPS origin). Set `GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET`. The app requests `gmail.readonly`, offline access, state protection, and PKCE. Public use of restricted Gmail scopes requires applicable Google verification and security assessment.
- Microsoft: register an application supporting the needed work/personal accounts and register `http://localhost:3030/api/oauth/microsoft/callback`. Set `MICROSOFT_CLIENT_ID` and `MICROSOFT_CLIENT_SECRET`. The app requests delegated `Mail.Read`, `User.Read`, and `offline_access` with state and PKCE. Organization policies may require admin consent.
- Connect under Email connections after reading the notice and granting separate email-processing consent. First sync looks back 30 days. Each mailbox is then checked on its own schedule (default every five minutes, `INVOICE_POLL_SECONDS` 60 to 3600; start times are spread so mailboxes do not all fire together, and a failing connection backs off up to an hour). For Gmail a quiet check is a single cheap history call that returns nothing when no mail has arrived; only when something has arrived does it run the usual filtered listing, and only matching PDFs are ever fetched. A first scan or a large backlog is saved as it goes and resumes on the next check, and an expired history position starts a full scan again without re-importing anything. Outlook still uses an arrival-time filter with a two-hour overlap, because per-folder delta tracking could not be tested without a Microsoft account. Empty checks leave no activity entries, and a failure is reported once when it starts. No send/delete permission is requested. Mailbox permissions themselves are broader than the app's PDF filtering.
- One mailbox may serve every store in its workspace. Each PDF attachment is imported independently. Clearly separable multipage bundles are split into individual PDFs, with the whole original attachment retained and linked. Ambiguous bundles or missing store matches require review.

Provider adapter references: [Gmail attachments](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages.attachments/get), [Microsoft attachment listing](https://learn.microsoft.com/en-us/graph/api/message-list-attachments?view=graph-rest-1.0), and [Microsoft attachment downloads](https://learn.microsoft.com/en-us/graph/api/attachment-get?view=graph-rest-1.0). Microsoft attachment metadata is listed first and each PDF is fetched separately, keeping a multi-invoice email from becoming one oversized response.

Neither provider has been connected in this build, so live OAuth and remote mailbox behavior remain unverified. No emails have been read, sent, changed, or deleted during development.

## Data and permissions

Users join workspaces through explicit memberships. A workspace has multiple stores, email connections, invoices, and exports. Owners manage stores, connections, and email-bound one-time invitation links; members can access all stores and invoice records in their workspace. A user may belong to multiple workspaces. Per-branch access restrictions are not implemented; invitations clearly disclose workspace-wide access.

Original PDFs, extracted invoice JSON, provider tokens, and generated workbooks use authenticated AES-256-GCM encryption bound to the workspace. Passwords use Argon2; session cookies are HttpOnly, SameSite=Lax, and Secure when an HTTPS origin is configured. Mutations require a session, CSRF token, and matching Origin. API queries obtain workspace context from a verified membership, not a caller-supplied workspace parameter.

The default encryption key is a private local file alongside the development database; INVOICE_ENCRYPTION_KEY_FILE can point to a separately mounted 32-byte key file. Account email, workspace/store names, consent history, and basic record metadata remain readable in SQLite. Before hosting customer data, use separately managed keys, encrypted disks/backups, restricted worker containers, and tested operational deletion/backup procedures. Gold was previously reported with FileVault and ownership enforcement disabled; code delivery does not change that configuration.

## Invoice logic

- Prices use integer cents and accept decimal commas/grouping spaces.
- The four supplied supplier layouts have deterministic parsers. Every other layout goes through a template-independent reader (`src/layout.rs`, `src/supplier.rs`): text is read as cells with column positions, so a value is found beside, inside, or below its label wherever a supplier put it, and the supplier is chosen by scoring every candidate name (legal suffix, an address, tax number or contact details lined up with it, "pay to" lines) while treating anything under "Bill to / Ship to", or containing a store name or address registered in the workspace, as the customer. An explicit sender label ("From:", "Vendor", "Sold by", "Remit to") outweighs every other signal when a supplier prints one, though none of the five real suppliers tried so far do; they print an unlabelled letterhead. Email metadata is not used, because invoices are often forwarded. Weak or competing evidence leaves the field blank for review with possible names listed; it never silently guesses. A line-item "Total" column is never taken as the invoice total, and a total is preferred when it equals subtotal plus tax. With the hard-coded layouts switched off (`INVOICE_FORCE_GENERIC=1 sumline --inspect file.pdf`), the generic reader reproduced all supplied fields on the five real layouts tried so far; it was tuned on those same five, so accuracy on other suppliers is not yet established. Unknown layouts always go to review the first time. When a person approves an invoice, the supplier they confirmed is remembered under the identifiers found in the document (the tax registration number printed beside the supplier, and the normalised name), so later invoices from that supplier are recognised whatever their layout and stop being held as a new layout; dates, currency, totals, line items and store matching are all still checked, and identifiers that point at two different confirmed names are held instead of guessed.
- Line items for unknown layouts are read from the table itself (`src/items.rs`): the heading row is recognised, each cell is assigned to the nearest heading, a row is a line that carries an amount, and wrapped text above or below a row is attached to it. Column positions are re-read on every page, a "Gross" or total-with-tax column is never the line amount, billed quantities beat ordered ones, and the items must add up to the subtotal or the invoice is held with a message saying by how much they differ. On the five real layouts the generic reader matched the hard-coded parsers on 189 of 190 item fields (the one difference was the hard-coded parser appending footer text to a description).
- Review form fields use the right control: currency is a CAD/USD dropdown (checked again on the server), amounts are numeric fields in cents-precision, dates are date pickers, the supplier offers the workspace's existing suppliers as suggestions, and the PO number is length-limited.
- Dates use reviewed supplier conventions. For other suppliers, written-out dates ("Oct 29, 2026") are read directly; a numeric date is settled by another date on the same invoice that only one order can explain (a due date of 10/16/2026 can only be month/day). Otherwise an ambiguous date is held for review.
- Invoice total, invoice balance, and current payment state are distinct. No payment status is inferred or payment initiated.
- Exact document hashes deduplicate within a workspace. Conflicting supplier/invoice identifiers are held for review. Store assignment is required before spreadsheet inclusion.
- Specific configured store identifiers are matched against invoice text. Multiple matches are never guessed. Review can assign a store manually.
- Known, reconciled invoices are added automatically; review/processing/excluded/bundle rows are held out of the workbook. Exports include store names, record IDs, line items, and authenticated links to the original PDFs.
- PDFs are processed in a child process with a wall timeout and CPU/file limits, without inherited OAuth environment secrets. PDFs must be at most 20 MB and 50 pages. No embedded code or invoice links are executed. Full container isolation is a production requirement.
- Excel text is written as literal strings. Invoice text cannot become a formula or external connection. The generated workbook is macro-free.
- Documents remain encrypted in the database until an authenticated download. Plaintext temporary parser files are deleted after processing; use an encrypted temporary filesystem in production.

## Privacy

`src/privacy.rs` contains the versioned notice served by the app. Account processing and email-processing choices are separate. Consent records capture user, workspace, kind, notice version, grant time, and withdrawal time. Disconnecting removes stored credentials and records withdrawal; original invoices remain until explicitly deleted. Provider-side permission revocation remains available in Google/Microsoft settings.

Before customer launch, complete the operator/contact details, exact hosting/processor locations, policy review for launch jurisdictions, account closure, secure deletion and backup retention. Owners can remove workspace members and invalidate their sessions. The draft does not claim legal compliance or zero security risk.

Accounts use email and password. When a mail backend is configured (`RESEND_API_KEY`, or `SMTP_URL`, with `MAIL_FROM`), new accounts are emailed a verification link, an unverified account cannot accept a workspace invitation, and "Forgot your password?" sends a one-hour, single-use reset link that also signs the account out everywhere. Tokens are stored only as hashes, resends are rate limited, and the forgot-password reply is identical whether or not the address has an account. Without a mail backend (local development) none of this is active, and a public deployment refuses to start unless one is configured. Resend is sent over HTTPS, so it works where outbound SMTP is blocked; `sumline --test-email <address>` checks the setup. The privacy policy and terms are public pages at `/privacy` and `/terms`, filled in from the `INVOICE_OPERATOR_*` settings, and registration records consent to both. Deleting an account (Privacy and consent) permanently deletes the workspaces where the person is the only member and removes them from shared ones; deleted data can remain in backups until they expire. A public deployment requires an encryption key from `INVOICE_ENCRYPTION_KEY`, a signup code, a mail backend and the operator details, and refuses to start without them. MFA, account closure/deletion, per-user/store access restrictions and workspace-owner transfer are still pending. The Docker configuration has not been executed in this development session.

## Backups and operations

`scripts/backup.sh <directory> [keep]` writes a consistent snapshot of the live database (`--backup`), proves it can be restored (`--verify-backup` runs SQLite's integrity check and decrypts a stored document with the key), and prunes old snapshots. A snapshot does not contain the encryption key; keep that file separately, because snapshot and key together expose every document. `/healthz` and `sumline --healthcheck` report liveness for proxies and the container `HEALTHCHECK`. See [DEPLOYMENT.md](DEPLOYMENT.md) and [LAUNCH_CHECKLIST.md](LAUNCH_CHECKLIST.md).

## Validation

```sh
cargo test --locked
cargo run --locked --bin sumline -- --inspect /path/to/invoice.pdf
```

The CLI inspection prints extracted invoice fields without printing bank/payment sections or the complete PDF text. `tests/smoke.py` runs API checks and sample regression checks against a running local preview when `INVOICE_SAMPLE_DIR` is supplied. On this Mac, all 98 Rust tests (including router-level tests for verification, reset and invitation gating, and a backup/restore round trip), Clippy with warnings treated as errors, the release build, and end-to-end checks passed. The account-email flow was also exercised against a local SMTP sink; no real mail provider has been used. End-to-end checks cover workspace isolation, roles and member removal, independent routing of attachments, splitting a mixed-store PDF, consent and CSRF gates, duplicates, literal Excel text, and the four supplied invoice layouts. Live email integration and the Docker runtime remain unverified.

## Hosting direction

The included Dockerfile builds a Rust binary and installs Poppler in a minimal runtime. Set an HTTPS `INVOICE_ORIGIN`, persistent protected storage, and provider credentials behind a TLS reverse proxy. This version intentionally uses one SQLite-backed process; scheduled polling and leased extraction jobs resume after restart. For multiple server instances, move persistence and job claiming to PostgreSQL/a durable queue and replace local-file keys with managed key storage. Gmail Pub/Sub and Microsoft Graph subscriptions are later event-driven adapters; this build uses scheduled polling.

## This iteration

The interface removes development badges/setup prose from normal screens, uses consistent navigation icons, improves mobile workspace access, and provides guarded review dialogs with Save & next. Supplier, store, status, currency and invoice-date filters combine with month/year/custom periods, date/supplier/amount sorting, separate currency totals, and matching spreadsheet exports.

Extraction jobs persist atomically with documents and use bounded workers, leases, retries and crash recovery. Reviews use optimistic revisions and transactional request-key replay. Duplicate publication checks run inside the write transaction. Encrypted query/export caches invalidate on changes; workbook generation runs off the async runtime. See [ARCHITECTURE.md](ARCHITECTURE.md) and [RESEARCH.md](RESEARCH.md) for exact guarantees and remaining limits.

For the next Gmail test, follow [GMAIL_TESTING.md](GMAIL_TESTING.md) and use Live-Test.command. Its database/configuration are separate from the sample preview. Run `python3 tests/make_mail_fixtures.py` to create fictional invoices for two stores. No real email has yet been read by this app.

The approved Tick identity is applied using BRAND.md and brand/tokens.css. Archivo and Atkinson fonts are served locally; their licences and source hashes are in web/fonts/. Status icons, readable identifiers, checked-total rules, and spreadsheet formatting follow the supplied brand. See BRAND_IMPLEMENTATION.md for the implementation details.
