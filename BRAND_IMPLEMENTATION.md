# Tick identity implementation

The approved Tick identity is applied to SumLine, the name chosen by the owner. The original brand handoff is preserved in the workspace planning folder. Asset geometry, palette and font licences are unchanged; visible product labels use SumLine. src/branding.rs is the app display-name source.

- web/tokens.css is an exact copy of brand/tokens.css. The runtime uses its light palette; dark mode remains a future option.
- Both app lockups use the supplied small-size inline Tick for their 16–23px display size and the live Archivo wordmark. The mark has no interface tile. The adaptive SVG favicon is copied unchanged.
- Archivo, Atkinson Hyperlegible Next and Atkinson Hyperlegible Mono are self-hosted Latin variable WOFF2 subsets. web/fonts/SOURCES.json records official download URLs and SHA-256 hashes; each family includes its SIL Open Font License. Other scripts use system fallbacks. There are no runtime font-provider requests.
- Status chips retain the existing labels and pair the supplied tick icons with status-specific colours. Primary actions use Ledger. Keyboard focus uses a 2px Ledger outline.
- Invoice numbers, PO numbers and amounts use the mono family. Display dates use a month name, money has two decimal places, and negative amounts use a true minus sign followed by “credit”. Native date inputs retain the browser’s local edit controls.
- The closing rule appears under the selected validated totals and in the drawer for ready/approved invoices. Empty selections and invoices awaiting review receive no rule.
- XLSX headers use Ledger with white text, a neutral table style, real sortable Excel dates displayed as d mmm yyyy, and a plain minus for credits. Workbook fonts remain Excel’s defaults.

Validation and visual captures are recorded with the delivery. Gmail setup and live-test steps are in GMAIL_TESTING.md.

The navigation identifies the active workspace once, in its selector. A letter avatar opens an account menu with the full email and an explicit red Sign out action. Mobile uses a compact header and a modal navigation drawer; desktop keeps its persistent sidebar. A native confirmation dialog defaults to Cancel, supports Escape, and mentions unsaved changes when relevant. Tablet and mobile tables scroll inside their card without extending the page.
