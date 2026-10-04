# SumLine brand

Status: **approved 2026-10-03**. Name: **SumLine** (formerly InvoiceTracker). Logo: the Tick. Applied to the app.

This file is the implementation source of truth. Assets live in `brand/`. The visual brand book (private, owner access) is at https://claude.ai/artifact/D7aykGSkWhNmUSGoiGBTtU; where the two differ, this file wins.

## Idea

SumLine should feel like a careful bookkeeper: calm, exact, and showing its working. The identity borrows from three bookkeeping objects:

- **Columnar ledger pads**: pale green paper with printed green rules. These give the palette.
- **Audit tick marks**: the pencil marks auditors put beside figures they have checked. These give the logo and the status icons.
- **The sum line**: the double underline a bookkeeper draws under a column once the total is checked. It gives the product its name and the brand its one signature device, the *closing rule*.

Brand promises, used in product copy and marketing:

| Promise | Meaning |
| --- | --- |
| One invoice, one record | An email is an envelope; each invoice inside it is filed on its own. |
| Nothing guessed | Uncertain stores, dates, currencies and totals go to review with the reason shown. |
| Every cent accounted for | Amounts are whole cents, and totals are checked against their lines. |
| The original stays attached | Every row links back to the PDF it came from. |

Tagline: **Every invoice. A clear record.** Supporting lines: "Nothing guessed." (review contexts) and "Every cent accounted for." (spreadsheet and exports).

The owner’s direct naming instruction uses **SumLine**. App display naming is centralized in `src/branding.rs`; crate, binary, repository and data-directory identifiers remain stable.

## Name and wordmark

**SumLine** is the line drawn under a sum once it has been added up and checked. The logo is a tick resting on that line, and every approved total in the app gets one.

- Write **SumLine**: one word, capital S and L. Never "Sumline", "Sum Line" or "SUMLINE" in running text. Say it "SUM-line".
- Wordmark: Archivo at `font-stretch: 112%`, `letter-spacing: -0.018em`, with "Sum" at weight 760 and "Line" at weight 430. Markup: `<span class="wordmark"><b>Sum</b>Line</span>` (no space, so screen readers read one word).
- Horizontal lockup: mark, then wordmark. Mark height = 1.15 × the wordmark font size; gap = 0.42em; vertically centered.
- Underlined lockup (covers, sign-in panel, spreadsheet title): the wordmark with the closing rule beneath it, spanning the word. The name sits on its own sum line. It takes no mark.
- Availability on 2026-10-03: `sumline.ca` was unregistered and `sumline.com` was registered to someone else. No invoicing or accounting product named SumLine turned up in a web search. SumUp, a payments company with an invoicing product, shares the "Sum" prefix. A formal trademark search (CIPO, USPTO) has **not** been done; don't use ™ or ® until it has.

## Logo: the Tick

An auditor's tick whose point rests on a double underline: checked, then totaled. It is also the parent of the status icons.

| File | Use |
| --- | --- |
| `brand/mark.svg` | Default mark, Ledger `#225447`, 24×24 grid |
| `brand/mark-reversed.svg` | Pad `#F1F5EE`, for Ledger fills or dark backgrounds |
| `brand/mark-small.svg` | Small-size mark for 16–23px, strokes snapped to whole pixels |
| `brand/favicon.svg` | Small-size mark that switches to `#9AD3B9` in dark browser chrome |
| `brand/app-icon.svg` | Pad mark on a Ledger tile, 64×64, radius 14 |

Inline version for HTML, colored by `currentColor` (set `color: var(--mark)`):

```html
<svg class="brand-mark" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
  <path fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" d="M5 10.25L9.5 14.75L19.25 3.25"/>
  <path fill="currentColor" d="M2.5 15.5H21.5V18.25H2.5ZM2.5 19.75H21.5V22.5H2.5Z"/>
</svg>
```

Rules:

- **Sizes**: full mark from 24px up; small-size mark from 16 to 23px; never below 16px.
- **Clear space**: at least ¼ of the mark's height on every side.
- **Color**: Ledger on light grounds, `#9AD3B9` on dark grounds, Pad on Ledger fills. Tick and rule are always the same color.
- **Don't**: recolor the closing rule (ochre means "needs review"), put the mark inside a circle or badge, stretch it, tilt it, or add a tinted tile behind it in the interface. The tile belongs to the app icon only.
- The wordmark is live text. Outline it to paths in a design tool before using it outside the app.

## The closing rule

The double underline is the brand's signature and the meaning of the name. Use it once per view, under whatever is final: a checked total, an approved invoice, the underlined lockup.

- In CSS: `border-bottom: 5px double var(--ink)` under the figure (gives two 1–2px lines with a gap), or two bars of equal thickness.
- An invoice that **needs review gets no closing rule** until a person approves it.
- Never use it as decoration or as a section divider.

## Color

Tokens are in `brand/tokens.css`. Light is the product default; dark values are ready for a future dark mode under `[data-theme="dark"]`.

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `--pad` | `#F1F5EE` | `#0E1814` | Page background |
| `--sheet` | `#FCFDFB` | `#15221D` | Cards, tables, drawers, dialogs |
| `--rule` | `#CFDDD2` | `#26392F` | Hairlines, row dividers |
| `--rule-strong` | `#9DB8A5` | `#3A5246` | Emphasized dividers, drop zones |
| `--control-border` | `#78967F` | `#5E7A6C` | Input and button outlines |
| `--ink` | `#15291F` | `#E3ECE5` | Text and figures |
| `--ink-2` | `#4D6157` | `#9AB0A4` | Secondary text, labels |
| `--ledger` | `#225447` | `#8FD0B3` | Brand, primary actions, links, focus ring |
| `--on-ledger` | `#FFFFFF` | `#0E1814` | Text on Ledger fills |
| `--mint` | `#DCE8DE` | `#1D3229` | Selected rows, active navigation |
| `--mark` | `#225447` | `#9AD3B9` | The Tick mark |

Status colors are text/icon ink on their own tint:

| Token pair | Light | Dark | Use |
| --- | --- | --- | --- |
| `--ok` / `--ok-tint` | `#3B6B34` / `#E3EFDC` | `#9CCB8C` / `#1B2A19` | In spreadsheet, reviewed, checks passed |
| `--hold` / `--hold-tint` | `#8A5A12` / `#F7EBD2` | `#E2B262` / `#2E2413` | Needs review |
| `--flow` / `--flow-tint` | `#3C6589` / `#E4ECF4` | `#94B9DC` / `#17222E` | Processing, syncing |
| `--void` / `--void-tint` | `#5E6862` / `#EDEFEC` | `#A3ACA6` / `#1F2522` | Excluded, split attachment |
| `--stop` / `--stop-tint` | `#A3362C` / `#F9E4E0` | `#F0907F` / `#341B17` | Failures, destructive actions |

Rules:

- Ledger green is for the brand, primary actions and links. It never marks a status; "ready" uses the separate `--ok` green.
- A status always pairs its color with a tick mark and a word. Never rely on color alone.
- Every text pair above meets WCAG AA 4.5:1 in both themes. `--control-border` meets 3:1 against `--sheet`.
- Focus ring: `outline: 2px solid var(--ledger); outline-offset: 2px`.
- Rough share of a screen: Pad 56%, Sheet 22%, Ink 10%, Rule 5%, Ledger 5%, status colors 2%.

## Typography

| Role | Family | Settings |
| --- | --- | --- |
| Display and wordmark | Archivo (variable, `wdth` and `wght` axes) | `font-stretch: 112–118%`, weight 650–760; headlines and the wordmark only |
| Interface and reading | Atkinson Hyperlegible Next | Weights 400–650; all UI text |
| Amounts and identifiers | Atkinson Hyperlegible Mono | Weights 400–600; amounts, invoice numbers, PO numbers, record IDs |

Atkinson Hyperlegible keeps easily confused characters (0/O, 1/l/I, 5/S, 8/B) distinct, so invoice and PO numbers can't be misread.

| Style | Setting | Size / line height |
| --- | --- | --- |
| Display | Archivo 720, 118% | 56 / 56 |
| Title | Archivo 660, 112% | 32 / 36 |
| Heading | Atkinson Next 650 | 20 / 28 |
| Body | Atkinson Next 400 | 16 / 26 |
| Small | Atkinson Next 400 | 14 / 20 |
| Label | Atkinson Mono 600, uppercase, +0.08em | 12 / 16 |
| Amount | Atkinson Mono 500 | 15 / 20 |

All three families are under the SIL Open Font License. **Self-host them** (woff2) instead of loading Google Fonts at runtime, so the app makes no third-party requests. The generated Excel workbook keeps Excel's default font; brand fonts aren't installed on users' machines.

### Numbers and dates

| Kind | Write | Avoid | Why |
| --- | --- | --- | --- |
| Money | `$3,920.00`, plus `CAD` where a workspace has more than one currency and in spreadsheet headers | `3920` | Symbol, grouping, two decimals |
| Dates | `18 Sep 2026` | `09/10/26` | Numeric dates are ambiguous; the app holds them for review |
| Identifiers | `PO 00412` | `PO 412` | Keep every character as printed, including leading zeros |
| Credits | `−$12.00 credit` | `(12.00)` | True minus sign; brackets are accounting shorthand staff may not know |

## Icons: tick marks

Status icons are drawn like auditors' tick marks: 20px grid, 1.75 stroke, round caps and joins, `currentColor`. The sprite is `brand/tickmarks.svg`. Icons appear next to a word, never alone.

Keep the app's existing status labels:

| Status key | Label (keep) | Symbol | Colors |
| --- | --- | --- | --- |
| `ready` | In spreadsheet | `t-ready` | `--ok` on `--ok-tint` |
| `approved` | Reviewed | `t-approved` | `--ok` on `--ok-tint` |
| `review` | Needs review | `t-review` | `--hold` on `--hold-tint` |
| `processing` | Processing | `t-processing` | `--flow` on `--flow-tint` |
| `excluded` | Excluded | `t-excluded` | `--void` on `--void-tint` |
| `bundle` | Split attachment | `t-bundle` | `--void` on `--void-tint` |

Supporting symbols: `t-footed` (lines, tax and total add up), `t-filed` (assigned to a store), `t-source` (open the original PDF).

Status chip: inline-flex, 16px icon, 6px gap, padding `4px 11px 4px 8px`, fully rounded, 13px weight 650. Pills are reserved for status chips; everything else keeps the existing radii (6–7px controls, 8–10px cards).

The sidebar navigation icons are outside this spec. Keep the current glyphs, or redraw them in the same stroke style (20px, 1.75 stroke, round caps).

## Voice

SumLine writes like a careful bookkeeper leaving a note: plain words, the exact figure, the reason, and what to do next.

- Use the words store staff use: store, supplier, invoice, spreadsheet, original PDF, workspace. Avoid system words in the interface: routing, ingest, parse, entity, pipeline, job.
- Explain every hold, and say how to clear it.
- Claim only what has been checked. No "100% accurate", "bank-grade", "secure" without qualification, or "AI-powered".
- Amounts carry their currency; dates carry a month name.
- Controls say exactly what happens ("Download spreadsheet"), and confirmations echo it ("Spreadsheet downloaded").

| Write | Not |
| --- | --- |
| Held for review. The delivery address matches two stores. Choose one to file this invoice. | Error: routing conflict. |
| 4 invoices filed from 1 email. One is waiting for you. | Email processed successfully! |
| SumLine reads PDF attachments. It can't send, change or delete your email. | Bank-grade security for your inbox. |
| Download spreadsheet | Export data |

## Applying it to this app

The current UI already uses a forest green, so this is a refinement rather than a redesign. Suggested order:

1. **Product name**. Change the user-visible name from InvoiceTracker to SumLine. Keep it in as few places as possible.

   | Where | Today | Change |
   | --- | --- | --- |
   | `web/index.html` | `<title>InvoiceTracker</title>` and both `.brand` blocks | `SumLine` title and the wordmark markup (step 3) |
   | `src/privacy.rs` | `# InvoiceTracker privacy notice` | `# SumLine privacy notice`. Consent records store the notice version, so decide whether the rename needs a new version |
   | `src/main.rs` | `InvoiceTracker running at {origin}` | `SumLine running at {origin}` |
   | `README.md` | Title and prose | SumLine, keeping technical commands as they are |
   | Google and Microsoft OAuth app registrations | Operator configuration, not code | Register the consent-screen app name as SumLine |

   Leave internal identifiers as they are for now: the crate and binary `invoice-tracker` (`Cargo.toml`, `Cargo.lock`, `Dockerfile`, `Launch.command`, `tests/smoke.py`), the `INVOICE_*` environment variables, the `invoice-tracker.sqlite` file, the `~/Library/Application Support/InvoiceTracker` data folder, the `invoice-tracker.local` demo address, and `INITIAL-PLANNING.md` (a historical draft). Renaming the database file or data folder would orphan existing local data unless it is migrated, so treat that as a separate, deliberate change. The spreadsheet download can stay `Invoices.xlsx`.
2. **Colors** (`web/style.css`). Swap the `:root` values; the variable names can stay:

   | Today | Becomes | Note |
   | --- | --- | --- |
   | `--paper #f5f7f2` | `#F1F5EE` (pad) | |
   | `--white #fff` | `#FCFDFB` (sheet) | |
   | `--line #dce4de` | `#CFDDD2` (rule) | |
   | `--ink #182e28` | `#15291F` (ink) | |
   | `--muted #697b74` | `#4D6157` (ink-2) | Today's value is about 4.1:1 on the page, below AA for small text |
   | `--green #225447` | `#225447` (ledger) | Unchanged |
   | `--mint #e6eee8` | `#DCE8DE` (mint) | |
   | `--amber #99601e` | `#8A5A12` (hold) | |

   Then replace literal hex values in the stylesheet with tokens where they act as tints or borders. Specifically: badge colors (today's review badge text is about 3.9:1 and ready about 4.2:1 at 10px), input borders `#ced8d1` → `var(--control-border)`, and the focus outline `#a7c9b2` → the Ledger focus ring above.
3. **Logo** (`web/index.html`). Both `.brand` blocks (auth panel and sidebar) use `<span class="brand-mark">▤</span> InvoiceTracker`. Replace them with the inline Tick SVG plus `<span class="wordmark"><b>Sum</b>Line</span>`. Drop the tinted 35px tile behind the mark; size the mark at 1.15 × the brand text size in `--mark`.
4. **Favicon**. Copy `brand/favicon.svg` to `web/favicon.svg`. Serve it from `src/main.rs` next to the existing `/style.css` route (`include_str!`, `image/svg+xml`), and add `<link rel="icon" href="/favicon.svg" type="image/svg+xml">` to the head. The Docker build copies only `src/` and `web/`, so runtime assets must live in `web/`, not `brand/`.
5. **Fonts**. Add subsetted Latin woff2 files to `web/fonts/` and serve them with `include_bytes!` routes (`font/woff2`). Declare `@font-face` with `font-display: swap`; for Archivo also declare `font-stretch: 62% 125%` and `font-weight: 100 900`. Downloading the font files needs the owner's go-ahead.
6. **Status badges** (`web/app.js` and `.badge` styles). Add the tick-mark icon per status, as in the table above, using the new status colors.
7. **Closing rule in the invoice drawer**. Give the total a closing rule when the status is `ready` or `approved`, and none while it needs review.
8. **Dates** (`date()` in `web/app.js`). It currently formats with `en-CA`, giving "Sep 18, 2026". Switch to day-month-year with a month name ("18 Sep 2026"). Note that `en-GB` produces "Sept" in recent browsers; either is acceptable.
9. **Spreadsheet** (`src/spreadsheet.rs`):
   - Header fill `0x17483E` → `0x225447`, with bold white text.
   - Check how the table style (`TableStyle::Medium4`) renders. If its header or banding fights Ledger green, use a neutral style and set formats explicitly. Banding, if any, uses `#F1F5EE`.
   - Money format `#,##0.00;[Red](#,##0.00)` → `#,##0.00;-#,##0.00`. Red is reserved for failures, and brackets are avoided.
   - Date cells, where they are real dates, use `d mmm yyyy`.
   - If a totals row is added, give the total cell a single top border and a double bottom border (`FormatBorder::Double`).
10. **Dark mode** is not required now. The tokens are ready when it is.

Optional: give the sign-in story panel a faint ledger-ruled background (`repeating-linear-gradient` with 28px rows in `--rule`) instead of the flat `#dfe9df`, with the underlined lockup.

Out of scope: changing product behavior or status semantics, renaming internal identifiers or data paths (see step 1), loading fonts or assets from third-party hosts, PNG icon sets (generate from `brand/app-icon.svg` when an installable app or touch icon is needed).

After implementing, run the checks in `AGENTS.md` (`cargo fmt --check`, `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`) and `tests/smoke.py` against a local preview. Then look at the sign-in page, invoice list, review drawer and a downloaded spreadsheet at desktop, ≤1100px and ≤760px widths. If `DELIVERY-SHA256.txt` is regenerated, include `BRAND.md` and `brand/`.
