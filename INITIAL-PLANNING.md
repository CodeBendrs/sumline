# InvoiceTracker

Planning draft - 2026-10-03. No application, email connection, or spreadsheet automation has been implemented.

## Objective

Capture information from supplier invoices received by email and update a spreadsheet, while minimizing mailbox access and additional disclosure of invoice data.

## Recommended starting point

A local Mac application imports PDFs from a selected folder, extracts fields locally, presents uncertain results for review, and updates a local Excel workbook on Gold. Start with manual attachment saving or drag-and-drop. Add automatic email import after extraction is validated and an email provider is selected.

Suggested flow: selected PDF -> local extraction -> validation and duplicate check -> review if needed -> invoice ledger -> Excel export.

Keep a local ledger as the durable source of records. Export the workbook without overwriting user-entered payment information. Queue exports when Excel has the workbook locked, and pause safely when Gold is unavailable. Local processing runs only while the computer and volume are available.

## Options

| Option | Convenience | Privacy and access tradeoff |
| --- | --- | --- |
| Manual PDF import into a local app | Automatic extraction and spreadsheet update; user saves attachments | No mailbox permission or additional cloud processor; local files and PDF parsing still require protection |
| Local app reading a dedicated invoice mailbox | Automatic fetch while the Mac is running | Best balance for full automation; restrict access to a separate invoice account/mailbox and use read-only OAuth where supported |
| Workflow hosted by the existing email provider | Can run while the Mac is off; easier shared cloud spreadsheet | Invoice data and workflow history remain in that provider's cloud; permission, retention, sharing, licensing, and extraction capabilities need review |
| Third-party automation or invoice extraction service | Less custom development | Adds processors and copies of sensitive data; assess access scopes, retention, subprocessors, and deletion before selecting |

An email folder or Gmail label is a processing filter, not necessarily an authorization boundary. Standard background Gmail read-only access can view messages throughout the account. Microsoft attachment access requires Mail.Read, which is broader than basic mail metadata. Prefer a separate invoice account or a provider-supported mailbox restriction when unattended access is required.

## Findings from the supplied samples

All four PDFs contain extractable text. No cloud OCR is needed for these samples. This does not establish accuracy on future vendors, scanned documents, or layout changes.

| Supplier | Invoice identifier | Subtotal shown | Tax shown | Invoice total shown | Currency evidence |
| --- | --- | ---: | ---: | ---: | --- |
| EHPLabs International LLC | INVEHPCA124319 | 792.00 | 39.60 | 831.60 | CAD explicitly stated |
| BioReversa | 19829 | 1,939.80 | 97.00 | 2,036.80 | CAD explicitly stated |
| FIT FOODS LP | 219925 | 10,509.26 | 525.46 | 11,034.72 | CAD explicitly stated |
| Crest Fire Extinguishers Ltd. | 40989 | 155.00 | 7.75 | 162.75 | Dollar symbol only; confirm or set a reviewed supplier default |

Implementation must handle supplier-specific date conventions, French decimal commas and TPS labels, wrapped descriptions, zero-price replacement/sample lines, and multipage invoices. Preserve leading zeros in purchase-order numbers. Preserve the original date text and require review if its interpretation is uncertain. Allow documented tax rounding differences; BioReversa's shown tax is one cent above a single calculation on the subtotal.

Invoice totals and stated balances are different fields. Do not infer current payment status or remaining balance solely from an invoice PDF; record payment updates separately.

## Suggested workbook

An Invoices sheet with one row per invoice: supplier, invoice number, invoice date, due date, currency, subtotal, tax amounts by type, shipping/discount if present, invoice total, stated invoice balance, PO number, category, review status, payment status, amount paid, payment date, import time, and original PDF reference.

An optional Line Items sheet: invoice key, SKU, description, quantity, unit price, line amount, and tax label. Keep summary totals separate from line-item totals to avoid double counting. A Summary sheet can group reviewed invoices by supplier, month, due date, and payment status.

## Security and reliability requirements for a future build

- Process locally by default; external OCR or AI must be a separate explicit choice.
- Treat PDFs and email content as untrusted data. Restrict file types, sizes, pages, and parser resources; keep dependencies updated. Do not execute embedded content or follow payment links automatically. If AI is later used, invoice text cannot instruct it to call tools or change system behavior.
- Export invoice text as literal spreadsheet values, so strings beginning with formula characters cannot execute as formulas. Use a macro-free workbook and avoid external workbook connections.
- Use provider sign-in with least-privilege OAuth where available. Store tokens in macOS Keychain, keep them out of source code/logs, and provide a way to revoke access. Do not request send/delete permissions for an import-only app.
- Protect Gold, the Mac, temporary files, and backups appropriately with encryption and limited access. Encryption must be checked; no drive format or encryption change is authorized by this plan.
- Deduplicate using attachment hashes plus supplier/invoice identifiers. A repeated invoice identifier with different content needs review rather than silent replacement. Record import errors and retry safely.
- Start with review before posting. Later, permit automatic posting only for known formats that pass field and arithmetic checks; keep exceptions visible. Keep source references and an audit trail without logging full invoices or bank details.
- Keep bank details out of the tracking spreadsheet unless separately required; extracting a total must never initiate a payment.

## Decisions still needed

1. Email provider and whether a separate invoice mailbox/account is possible.
2. Local Excel on Gold or a shared cloud spreadsheet.
3. Invoice summary only or individual products/quantities as well.
4. Approximate monthly invoice volume and whether it must work while the Mac is off.

## Primary references checked

- Gmail access scopes: https://developers.google.com/workspace/gmail/api/auth/scopes
- Microsoft attachment permissions: https://learn.microsoft.com/en-us/graph/api/message-list-attachments?view=graph-rest-1.0
- Google Sheets per-file access scopes: https://developers.google.com/workspace/sheets/api/scopes
- Microsoft invoice extraction in Power Automate: https://learn.microsoft.com/en-us/ai-builder/prebuilt-invoice-processing
- Apple storage encryption guidance: https://support.apple.com/en-ie/guide/disk-utility/dskutl35612/mac
