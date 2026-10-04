pub const POLICY_VERSION: &str = "2026-10-03-preview-2";
pub const NOTICE: &str = r#"# {{PRODUCT_NAME}} privacy notice
Version: 2026-10-03-preview-2. Effective: 3 October 2026.

This notice describes the development preview. The service operator, production hosting location, processor contracts, and privacy contact must be configured before customer launch. This is a product-specific draft, not a statement that the service is legally ready to launch.

## Information collected
We collect your email address, workspace and store names, password hash, membership permissions, and consent choices. When you upload or connect email, we receive invoice PDFs and source metadata, such as sender, subject, and receipt/import time. Invoices can contain names, addresses, bank details, and other personal information. The original PDFs are retained so your authorized team can retrieve them. Extracted invoice fields include supplier, document numbers, dates, products, amounts, and tax.

## Email access and choices
Connecting email is optional. You may upload PDFs instead. Gmail read-only and Microsoft Mail.Read permissions can allow access across the selected mailbox; they are not restricted to one folder by the permission itself. The app searches for PDF attachments, downloads candidate documents, and does not retain email body text. Non-invoice PDFs can be received and are held or excluded after inspection. The app does not request permission to send or delete mail. You must have authority to connect that mailbox and share its invoices with your workspace.

## Purpose and recipients
We use these records to capture invoices, route them to stores, flag uncertainties, create spreadsheets, and maintain an accessible archive. Workspace owners and invited members can access all stores and invoices within that workspace. You control whom you invite. This preview uses no advertising, analytics service, external AI extraction, or sale of invoice information. Google or Microsoft supplies email data when you connect its service. The development server stores the data; production hosting and any additional processors must be named in an updated notice before use.

## Storage and protection
Original PDFs, extracted invoice records, workbooks, and provider tokens are encrypted by the application. Account identifiers, email addresses, store names, access metadata, and consent history are stored in the database. Passwords are hashed. Encrypted retry receipts for invoice saves are kept for up to 24 hours. Keys normally reside on the development server, with an option to load a separately mounted key file; production must use separately managed keys and protected storage/backups. Encryption does not make a compromised server harmless. This preview is bound to localhost and is not a public service.

## Retention and control
Invoices remain available until an authorized workspace member deletes them. Removing an email connection deletes locally stored provider tokens and stops future checks; already imported invoices remain until you delete them. You may also revoke the app in your provider's account settings. Deleting an invoice removes its active database copy, clears cached results and retry receipts, and invalidates cached workbooks so subsequent exports reflect the deletion. When an attachment contains several invoices, the full attachment and its extracted invoice PDFs are separate archive records. Deleting an extracted invoice does not remove it from the full attachment; delete that source attachment as well to remove both active copies. Deleting the source attachment leaves its extracted invoices available. This preview has no automated backup service; SQLite storage and previous user-downloaded exports may still contain residual copies. Production backup retention and secure deletion procedures must be established before launch. Consent records record the notice version and grant/withdrawal time; no IP address is collected for consent.

## Your requests and contact
You can download original invoices and spreadsheet data, correct extraction, remove a connection, or delete invoices through the app. For account closure, access/correction requests, or concerns, contact the operator who supplied this preview. Before production use, the operator must replace this contact information with its legal identity and privacy contact and define a supported account-deletion process.

## Changes
Material changes to processing, recipients, or purpose require a new notice and renewed applicable consent. Optional email consent is presented separately from account creation. A checkbox acknowledging this notice is not a substitute for the operator's other privacy obligations.
"#;
