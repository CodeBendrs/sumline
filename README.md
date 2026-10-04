# SumLine

A Rust web application for capturing supplier invoices from existing Gmail or Outlook mailboxes, routing individual invoices to stores, reviewing uncertain results, and maintaining downloadable Excel workbooks and an original-document archive.

This is a working development build, not a launched customer service. Email adapters are implemented but require the operator's OAuth credentials and real-account validation. The preview privacy notice describes the current code and is explicitly a draft pending the operator's legal identity, contact, hosting, and retention decisions.

## Run

Requirements: Rust 1.97 or later. Install Poppler (`brew install poppler` on Mac, `apt-get install poppler-utils` on Linux). Poppler is the preferred PDF reader because one supplied invoice has a malformed cross-reference table that the Rust-only fallback cannot read.

```sh
cp .env.example .env
cargo run --locked --bin invoice-tracker
```

Open http://localhost:3030. Create an account and a first store. In Stores, add a specific address or store identifier from its invoices. Add more stores as needed. Upload PDFs or configure and connect email. Review uncertain records, then export a workbook for the selected supplier, store and date range. Original PDFs are available from each invoice or the Document archive.

On this Mac, `Launch.command` starts the supplied native binary when present, or builds the source with Cargo. It defaults to storing application data under `~/Library/Application Support/InvoiceTracker`, keeping runtime data separate from the source folder on Gold. The browser opens at the URL printed in the terminal; the launcher does not connect a mailbox or enable the sample login. The app maintains its own XLSX workbook; Google Sheets/Excel Online synchronization is not implemented.

Optional sample preview: set `INVOICE_DEMO=1` and `INVOICE_SAMPLE_DIR` to your local directory containing the four supplied PDFs. The sample workspace is only allowed when the server binds to loopback. Sample PDFs, databases, encryption keys, and OAuth secrets are excluded from Git.

## Existing email integration

The operator registers applications once; each store owner connects their existing mailbox through the provider's consent screen. Suppliers do not change email addresses or contact details.

- Google: enable Gmail API, create a web OAuth client, and register `http://localhost:3030/api/oauth/google/callback` (or your HTTPS origin). Set `GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET`. The app requests `gmail.readonly`, offline access, state protection, and PKCE. Public use of restricted Gmail scopes requires applicable Google verification and security assessment.
- Microsoft: register an application supporting the needed work/personal accounts and register `http://localhost:3030/api/oauth/microsoft/callback`. Set `MICROSOFT_CLIENT_ID` and `MICROSOFT_CLIENT_SECRET`. The app requests delegated `Mail.Read`, `User.Read`, and `offline_access` with state and PKCE. Organization policies may require admin consent.
- Connect under Email connections after reading the notice and granting separate email-processing consent. First sync looks back 30 days; scheduled checks run every five minutes while the server is running. No send/delete permission is requested. Mailbox permissions themselves are broader than the app's PDF filtering.
- One mailbox may serve every store in its workspace. Each PDF attachment is imported independently. Clearly separable multipage bundles are split into individual PDFs, with the whole original attachment retained and linked. Ambiguous bundles or missing store matches require review.

Provider adapter references: [Gmail attachments](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages.attachments/get), [Microsoft attachment listing](https://learn.microsoft.com/en-us/graph/api/message-list-attachments?view=graph-rest-1.0), and [Microsoft attachment downloads](https://learn.microsoft.com/en-us/graph/api/attachment-get?view=graph-rest-1.0). Microsoft attachment metadata is listed first and each PDF is fetched separately, keeping a multi-invoice email from becoming one oversized response.

Neither provider has been connected in this build, so live OAuth and remote mailbox behavior remain unverified. No emails have been read, sent, changed, or deleted during development.

## Data and permissions

Users join workspaces through explicit memberships. A workspace has multiple stores, email connections, invoices, and exports. Owners manage stores, connections, and email-bound one-time invitation links; members can access all stores and invoice records in their workspace. A user may belong to multiple workspaces. Per-branch access restrictions are not implemented; invitations clearly disclose workspace-wide access.

Original PDFs, extracted invoice JSON, provider tokens, and generated workbooks use authenticated AES-256-GCM encryption bound to the workspace. Passwords use Argon2; session cookies are HttpOnly, SameSite=Lax, and Secure when an HTTPS origin is configured. Mutations require a session, CSRF token, and matching Origin. API queries obtain workspace context from a verified membership, not a caller-supplied workspace parameter.

The default encryption key is a private local file alongside the development database; INVOICE_ENCRYPTION_KEY_FILE can point to a separately mounted 32-byte key file. Account email, workspace/store names, consent history, and basic record metadata remain readable in SQLite. Before hosting customer data, use separately managed keys, encrypted disks/backups, restricted worker containers, and tested operational deletion/backup procedures. Gold was previously reported with FileVault and ownership enforcement disabled; code delivery does not change that configuration.

## Invoice logic

- Prices use integer cents and accept decimal commas/grouping spaces.
- The four supplied supplier layouts have deterministic parsers; unknown layouts remain reviewable.
- Dates use reviewed supplier conventions. Ambiguous unknown-vendor dates are held for review.
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

Account registration currently uses email/password without email verification, password recovery, or MFA. Use the preview with trusted test accounts; production invitations must depend on verified identities. Per-user/store access restrictions and workspace-owner transfer are also pending. The Docker configuration has not been executed in this development session.

## Validation

```sh
cargo test --locked
cargo run --locked --bin invoice-tracker -- --inspect /path/to/invoice.pdf
```

The CLI inspection prints extracted invoice fields without printing bank/payment sections or the complete PDF text. `tests/smoke.py` runs API checks and sample regression checks against a running local preview when `INVOICE_SAMPLE_DIR` is supplied. On this Mac, all sixteen Rust tests, Clippy with warnings treated as errors, the release build, and end-to-end checks passed. End-to-end checks cover workspace isolation, roles and member removal, independent routing of attachments, splitting a mixed-store PDF, consent and CSRF gates, duplicates, literal Excel text, and the four supplied invoice layouts. Live email integration and the Docker runtime remain unverified.

## Hosting direction

The included Dockerfile builds a Rust binary and installs Poppler in a minimal runtime. Set an HTTPS `INVOICE_ORIGIN`, persistent protected storage, and provider credentials behind a TLS reverse proxy. This version intentionally uses one SQLite-backed process; scheduled polling and leased extraction jobs resume after restart. For multiple server instances, move persistence and job claiming to PostgreSQL/a durable queue and replace local-file keys with managed key storage. Gmail Pub/Sub and Microsoft Graph subscriptions are later event-driven adapters; this build uses scheduled polling.

## This iteration

The interface removes development badges/setup prose from normal screens, uses consistent navigation icons, improves mobile workspace access, and provides guarded review dialogs with Save & next. Supplier, store, status, currency and invoice-date filters combine with month/year/custom periods, date/supplier/amount sorting, separate currency totals, and matching spreadsheet exports.

Extraction jobs persist atomically with documents and use bounded workers, leases, retries and crash recovery. Reviews use optimistic revisions and transactional request-key replay. Duplicate publication checks run inside the write transaction. Encrypted query/export caches invalidate on changes; workbook generation runs off the async runtime. See [ARCHITECTURE.md](ARCHITECTURE.md) and [RESEARCH.md](RESEARCH.md) for exact guarantees and remaining limits.

For the next Gmail test, follow [GMAIL_TESTING.md](GMAIL_TESTING.md) and use Live-Test.command. Its database/configuration are separate from the sample preview. Run `python3 tests/make_mail_fixtures.py` to create fictional invoices for two stores. No real email has yet been read by this app.

The approved Tick identity is applied using BRAND.md and brand/tokens.css. Archivo and Atkinson fonts are served locally; their licences and source hashes are in web/fonts/. Status icons, readable identifiers, checked-total rules, and spreadsheet formatting follow the supplied brand. See BRAND_IMPLEMENTATION.md for the implementation details.
