use crate::layout::{
    Hit, How, Layout, accept_date, accept_identifier, accept_money, accept_reference,
    find_labelled, named_date,
};
use crate::model::{Invoice, LineItem, parse_money};
use crate::supplier;
use chrono::NaiveDate;
use regex::Regex;

/// What the parser may know about the reader of the invoice. The customer's own names and
/// addresses are evidence about who the supplier is not.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// Lower-case, whitespace-collapsed store names and identifiers.
    pub customer: Vec<String>,
    /// Ignore the hard-coded supplier recognisers, to measure the template-independent engine.
    pub force_generic: bool,
    /// Identifier -> supplier name that a person confirmed earlier in this workspace.
    pub known_suppliers: std::collections::HashMap<String, String>,
}

/// What earlier approvals say about the supplier of this document.
enum Remembered {
    No,
    As(String),
    /// The document's identifiers point at different confirmed names.
    Conflict,
}

fn remembered(keys: &[String], known: &std::collections::HashMap<String, String>) -> Remembered {
    let mut names: Vec<&String> = keys.iter().filter_map(|k| known.get(k)).collect();
    names.sort();
    names.dedup();
    match names.as_slice() {
        [] => Remembered::No,
        [one] => Remembered::As((*one).clone()),
        _ => Remembered::Conflict,
    }
}

/// First value attached to any of the labels: inline text beats an adjacent cell, which beats a
/// value read from the row below a header.
fn engine_text(
    layout: &Layout,
    labels: &str,
    accept: &dyn Fn(&str) -> Option<String>,
    allow_below: bool,
) -> Option<String> {
    let hits = find_labelled(layout, labels, false, accept);
    [How::Inline, How::Right, How::Below]
        .into_iter()
        .filter(|how| allow_below || *how != How::Below)
        .find_map(|how| hits.iter().find(|h| h.how == how))
        .map(|h| h.value.clone())
}

/// Only "label: value" written inside a single cell, such as "Invoice 40989".
fn inline_only(
    layout: &Layout,
    labels: &str,
    accept: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    find_labelled(layout, labels, false, accept)
        .into_iter()
        .find(|h| h.how == How::Inline)
        .map(|h| h.value)
}

/// A bare "Date" label. Next to its value it is the invoice date; as a column heading it counts
/// only on a header row that also carries other invoice headings, so a line-item date column
/// is not mistaken for it.
fn bare_date_label(layout: &Layout) -> Option<String> {
    let companions =
        Regex::new(r"(?i)\b(?:due|invoice|please pay|amount|terms|balance)\b").unwrap();
    find_labelled(layout, "date", false, &accept_date)
        .into_iter()
        .find(|h| {
            h.how != How::Below
                || layout.lines[h.line]
                    .cells
                    .iter()
                    .any(|c| companions.is_match(&c.text))
        })
        .map(|h| h.value)
}

/// Amounts attached to a label. A figure read from below a heading is dropped when that heading
/// belongs to a line-item table: its "Total" or "Amount" column holds per-line amounts, never
/// the document's total.
fn money_hits(layout: &Layout, labels: &str) -> Vec<Hit> {
    let table_heading = Regex::new(
        r"(?i)^(?:description|qty|quantity|item(?:\s*code)?|unit\s*price|price|rate|ordered|shipped|uom|sku|code)$",
    )
    .unwrap();
    find_labelled(layout, labels, true, &accept_money)
        .into_iter()
        .filter(|h| {
            h.how != How::Below
                || !layout.lines[h.line]
                    .cells
                    .iter()
                    .any(|c| table_heading.is_match(&c.text))
        })
        .collect()
}

/// The last amount attached to a label. Totals come after the table, so the last one wins; a
/// header-over-values reading is used only when nothing sits beside a label.
fn engine_money(layout: &Layout, labels: &str) -> Option<i64> {
    let hits = money_hits(layout, labels);
    let pick = |wanted: &dyn Fn(&Hit) -> bool| {
        hits.iter()
            .rfind(|h| wanted(h))
            .and_then(|h| parse_money(&h.value))
    };
    pick(&|h| h.how != How::Below).or_else(|| pick(&|h| h.how == How::Below))
}

fn capture(text: &str, pattern: &str) -> Option<String> {
    Regex::new(pattern)
        .ok()?
        .captures(text)?
        .get(1)
        .map(|v| v.as_str().trim().to_string())
}

fn labeled_money(text: &str, labels: &[&str]) -> Option<i64> {
    for label in labels {
        let pattern = format!(
            r"(?im)(?:^|[ \t▌]){}\s*[:\s]*(?:\(?CAD\)?\s*)?\$?\s*(-?\d[\d ,]*[.,]\d{{2}})\s*(?:CAD)?\s*$",
            label
        );
        if let Some(value) = capture(text, &pattern).and_then(|v| parse_money(&v)) {
            return Some(value);
        }
    }
    None
}

fn date_candidates(raw: &str) -> Vec<NaiveDate> {
    let mut result = Vec::new();
    if let Some(date) = named_date(raw) {
        return vec![date];
    }
    for format in [
        "%Y/%m/%d", "%Y-%m-%d", "%d/%m/%Y", "%m/%d/%Y", "%d-%m-%Y", "%m-%d-%Y",
    ] {
        if let Ok(date) = NaiveDate::parse_from_str(raw, format)
            && !result.contains(&date)
        {
            result.push(date);
        }
    }
    result
}

fn dates(invoice: &mut Invoice, convention: Option<&str>, net_days: Option<i64>) {
    // One document uses one convention. If exactly one numeric order fits every date present
    // (a due date of 10/16/2026 can only be month/day), it settles the others.
    let inferred = if convention.is_none() {
        let present: Vec<&str> = [&invoice.original_invoice_date, &invoice.original_due_date]
            .into_iter()
            .map(String::as_str)
            .filter(|d| !d.is_empty() && named_date(d).is_none())
            .collect();
        let fits: Vec<&str> = [
            "%Y/%m/%d", "%Y-%m-%d", "%d/%m/%Y", "%m/%d/%Y", "%d-%m-%Y", "%m-%d-%Y",
        ]
        .into_iter()
        .filter(|f| {
            present
                .iter()
                .all(|d| NaiveDate::parse_from_str(d, f).is_ok())
        })
        .collect();
        // Slash and dash spellings of one order count as the same convention.
        let orders: std::collections::HashSet<String> =
            fits.iter().map(|f| f.replace('-', "/")).collect();
        (!present.is_empty() && orders.len() == 1).then(|| fits[0])
    } else {
        None
    };
    let convention = convention.or(inferred);
    let parse = |raw: &str| convention.and_then(|fmt| NaiveDate::parse_from_str(raw, fmt).ok());
    let (issue, due) = if convention.is_some() {
        (
            parse(&invoice.original_invoice_date),
            parse(&invoice.original_due_date),
        )
    } else {
        let issues = date_candidates(&invoice.original_invoice_date);
        let dues = date_candidates(&invoice.original_due_date);
        let pairs: Vec<_> = issues
            .iter()
            .flat_map(|a| dues.iter().map(move |b| (*a, *b)))
            .filter(|(a, b)| net_days.is_some_and(|days| (*b - *a).num_days() == days))
            .collect();
        if pairs.len() == 1 {
            (Some(pairs[0].0), Some(pairs[0].1))
        } else {
            (
                if issues.len() == 1 {
                    Some(issues[0])
                } else {
                    None
                },
                if dues.len() == 1 { Some(dues[0]) } else { None },
            )
        }
    };
    invoice.invoice_date = issue.map(|v| v.format("%Y-%m-%d").to_string());
    invoice.due_date = due.map(|v| v.format("%Y-%m-%d").to_string());
}

pub fn validate(invoice: &Invoice) -> Vec<String> {
    let mut issues = Vec::new();
    if invoice.supplier.trim().is_empty() {
        issues.push("Confirm the supplier.".into());
    }
    if invoice.invoice_number.trim().is_empty() {
        issues.push("Confirm the invoice number.".into());
    }
    if invoice.invoice_date.is_none() {
        issues.push("Confirm the invoice date; its format is unclear.".into());
    }
    if invoice.due_date.is_none() {
        issues.push("Confirm the due date or leave it explicitly blank during review.".into());
    }
    if invoice.currency.is_none() {
        issues.push("Confirm the currency; it is not printed on this invoice.".into());
    }
    if invoice.total_cents.is_none() {
        issues.push("Confirm the invoice total.".into());
    }
    if invoice.total_cents.is_some_and(|v| v < 0) {
        issues.push("Check this negative total and whether the document is a credit note.".into());
    }
    match (
        invoice.subtotal_cents,
        invoice.tax_cents,
        invoice.total_cents,
    ) {
        (Some(a), Some(b), Some(c)) if (a + b - c).abs() > 1 => issues.push(
            "Subtotal plus tax does not match the total. Check shipping, discounts, or extraction."
                .into(),
        ),
        (None, _, _) | (_, None, _) => issues.push("Confirm the subtotal and tax.".into()),
        _ => {}
    }
    if invoice
        .items
        .iter()
        .any(|item| item.amount_cents.is_some_and(|c| c < 0))
    {
        issues.push(
            "This document includes negative line amounts. Check whether it is a credit note."
                .into(),
        );
    }
    issues
}

pub fn parse_invoice(text: &str, pages: usize) -> Invoice {
    parse_invoice_with(text, pages, &Context::default())
}

pub fn parse_invoice_with(text: &str, pages: usize, context: &Context) -> Invoice {
    let text = crate::layout::clean(text);
    let mut invoice = Invoice {
        pages,
        parser: "generic".into(),
        ..Default::default()
    };
    if text.trim().len() < 30 {
        invoice.warnings.push(
            "This PDF has no readable text. OCR is needed; no external service was contacted."
                .into(),
        );
        return invoice;
    }
    let is_ehp = !context.force_generic && text.contains("EHPLabs");
    let is_bio = !context.force_generic && text.contains("BioReversa");
    let is_fit = !context.force_generic && text.contains("FIT FOODS");
    let is_crest = !context.force_generic && text.contains("Crest Fire");
    let known = is_ehp || is_bio || is_fit || is_crest;
    let layout = Layout::new(&text);
    let guess = if known {
        supplier::Guess::default()
    } else {
        supplier::detect(&layout, &context.customer)
    };
    let identity_keys = supplier::identity_keys(&guess);
    let memory = remembered(&identity_keys, &context.known_suppliers);
    invoice.identity_keys = identity_keys;
    invoice.supplier = if is_ehp {
        "EHPLabs International LLC".into()
    } else if is_bio {
        "BioReversa".into()
    } else if is_fit {
        "FIT FOODS LP".into()
    } else if is_crest {
        "Crest Fire Extinguishers Ltd.".into()
    } else if let Remembered::As(name) = &memory {
        invoice.remembered_supplier = true;
        name.clone()
    } else {
        guess.name.clone().unwrap_or_default()
    };
    if is_ehp {
        invoice.parser = "ehplabs-v1".into();
    }
    if is_bio {
        invoice.parser = "bioreversa-v1".into();
    }
    if is_fit {
        invoice.parser = "fitfoods-v1".into();
    }
    if is_crest {
        invoice.parser = "crest-v1".into();
    }
    invoice.invoice_number = capture(
        &text,
        r"(?i)(?:invoice\s*(?:number|no\.?|#)|facture\s*#)\s*:?\s*([A-Z0-9][A-Z0-9_-]*)",
    )
    .or_else(|| capture(&text, r"(?i)Invoice\s+(\d{3,})"))
    .unwrap_or_default();
    invoice.original_invoice_date = issue_date(&text).unwrap_or_default();
    invoice.original_due_date = capture(
        &text,
        r"(?i)Due Date\s*:?\s*(\d{1,4}[/-]\d{1,2}[/-]\d{2,4})",
    )
    .unwrap_or_default();
    if is_ehp && invoice.original_due_date.is_empty() {
        invoice.original_due_date =
            capture(&text, r"(?i)Net\s+30\s+(\d{2}/\d{2}/\d{4})").unwrap_or_default();
    }
    if is_crest {
        // This supplier places both dates under a combined column heading.
        let all: Vec<_> = Regex::new(r"\b\d{2}/\d{2}/\d{4}\b")
            .unwrap()
            .find_iter(&text)
            .map(|v| v.as_str().to_string())
            .collect();
        if all.len() >= 2 {
            invoice.original_invoice_date = all[0].clone();
            invoice.original_due_date = all[1].clone();
        }
    }
    if !known {
        const NUMBER: &str = r"invoice\s*(?:number|no|num|id)|inv\s*(?:no|#|number)|facture\s*(?:no|#|number)|num[ée]ro\s+de\s+facture|no\.?\s+de\s+facture|bill\s*(?:number|no)";
        const ISSUE: &str = r"invoice\s*date|issue\s*date|date\s*issued|date\s*of\s*issue|document\s*date|billing\s*date|date\s*de\s*facture";
        const DUE: &str = r"due\s*date|payment\s*due(?:\s*date)?|date\s*due|due\s*on|pay\s*by|date\s*d['’]?[ée]ch[ée]ance|[ée]ch[ée]ance|due";
        invoice.invoice_number = engine_text(&layout, NUMBER, &accept_identifier, true)
            .or_else(|| inline_only(&layout, "invoice|facture", &accept_identifier))
            .or_else(|| accept_identifier(&invoice.invoice_number))
            .unwrap_or_default();
        invoice.original_invoice_date = engine_text(&layout, ISSUE, &accept_date, true)
            .or_else(|| bare_date_label(&layout))
            .or_else(|| accept_date(&invoice.original_invoice_date))
            .unwrap_or_default();
        invoice.original_due_date = engine_text(&layout, DUE, &accept_date, true)
            .or_else(|| accept_date(&invoice.original_due_date))
            .unwrap_or_default();
    }
    let convention = if is_ehp {
        Some("%d/%m/%Y")
    } else if is_bio {
        Some("%Y/%m/%d")
    } else if is_fit || is_crest {
        Some("%m/%d/%Y")
    } else {
        None
    };
    let net = capture(&text, r"(?i)Net\s+(\d+)").and_then(|v| v.parse::<i64>().ok());
    dates(&mut invoice, convention, net);
    invoice.currency = if Regex::new(r"(?i)\bCAD\b|Canadian Dollars")
        .unwrap()
        .is_match(&text)
    {
        Some("CAD".into())
    } else if Regex::new(r"(?i)\bUSD\b|US Dollars")
        .unwrap()
        .is_match(&text)
    {
        Some("USD".into())
    } else {
        None
    };
    invoice.subtotal_cents = labeled_money(
        &text,
        &[
            r"SUB-TOTAL\(CAD\)",
            "SUBTOTAL",
            "SUB-TOTAL",
            "Total Before Tax",
        ],
    );
    invoice.tax_cents = labeled_money(
        &text,
        &[
            "TOTAL GST",
            "Total Tax Amount",
            "TPS @ 5%",
            "GST @ 5%",
            "TOTAL TAX",
        ],
    );
    invoice.total_cents = labeled_money(&text, &["TOTAL CAD", "Total Amount", "TOTAL"]);
    invoice.balance_cents = labeled_money(&text, &["Amount Due CAD", "BALANCE DUE", "TOTAL DUE"]);
    invoice.po_number = capture(
        &text,
        r"(?i)(?:Customer PO|PO#|P\.O\.\s*(?:number|no\.?))\s*:?\s*([A-Z0-9_-]+)",
    );
    if is_ehp {
        invoice.po_number = capture(&text, r"(?i)Net\s+30\s+\d{2}/\d{2}/\d{4}\s+(.+?)\s+Caitlan");
    }
    if is_bio {
        invoice.po_number = capture(&text, r"(?i)\d{4}/\d{2}/\d{2}\s+ups\s+\S+\s+(\d+)");
    }
    if !known {
        const SUBTOTAL: &str = r"sub-?\s?total|total\s+before\s+tax(?:es)?|amount\s+before\s+tax(?:es)?|net\s+(?:amount|total)|merchandise\s+total|goods\s+total";
        const TAX_TOTAL: &str = r"total\s+(?:sales\s+)?tax(?:es)?|tax(?:es)?\s+total|total\s+(?:gst|hst|vat|gst/hst)|sales\s+tax\s+total";
        const TAX_LINE: &str = r"(?:gst|hst|pst|qst|tps|tvq|vat)(?:\s*(?:@\s*)?\(?\d+(?:\.\d+)?\s*%\)?)?|sales\s+tax|tax";
        const TOTAL_STRONG: &str = r"grand\s+total|invoice\s+total|total\s+invoice|total\s+amount|amount\s+total|total\s+\(?(?:cad|usd)\)?|montant\s+total|total\s+[àa]\s+payer";
        const BALANCE: &str =
            r"amount\s+due|balance\s+due|total\s+due|balance|amount\s+owing|please\s+pay";
        invoice.subtotal_cents = engine_money(&layout, SUBTOTAL).or(invoice.subtotal_cents);
        invoice.balance_cents = engine_money(&layout, BALANCE).or(invoice.balance_cents);
        let totals: Vec<i64> = money_hits(&layout, TOTAL_STRONG)
            .into_iter()
            .chain(money_hits(&layout, "total"))
            .filter(|h| h.how != How::Below)
            .filter_map(|h| parse_money(&h.value))
            .collect();
        invoice.total_cents = totals
            .last()
            .copied()
            .or_else(|| engine_money(&layout, TOTAL_STRONG))
            .or_else(|| engine_money(&layout, "total"))
            .or(invoice.total_cents);
        invoice.tax_cents = engine_money(&layout, TAX_TOTAL).or_else(|| {
            // No tax total: add the individual tax lines, but only if that reconciles the
            // invoice. A sum that does not is evidence of a misread, not a tax figure.
            let lines: Vec<i64> = find_labelled(&layout, TAX_LINE, true, &accept_money)
                .iter()
                .filter(|h| h.how != How::Below)
                .filter_map(|h| parse_money(&h.value))
                .collect();
            let sum: i64 = lines.iter().sum();
            match (invoice.subtotal_cents, invoice.total_cents) {
                (Some(s), Some(t)) if !lines.is_empty() && (s + sum - t).abs() <= 1 => Some(sum),
                _ => None,
            }
            .or(invoice.tax_cents)
        });
        invoice.po_number = engine_text(
            &layout,
            r"po\s*(?:number|no|#)|p\.?o\.?\s*(?:number|no|#)|purchase\s+order(?:\s+(?:number|no|#))?|customer\s+po|po",
            &accept_reference,
            true,
        )
        .or(invoice.po_number.clone());
        // Several amounts may be labelled "total" (a column total, a page total). The one that
        // equals subtotal plus tax is the invoice total.
        if let (Some(s), Some(t)) = (invoice.subtotal_cents, invoice.tax_cents)
            && let Some(matching) = totals.iter().rev().find(|v| (s + t - **v).abs() <= 1)
        {
            invoice.total_cents = Some(*matching);
        }
    }
    parse_items(&text, &mut invoice);
    if !known {
        let found = crate::items::parse(&layout);
        if !found.is_empty() {
            let sum: i64 = found.iter().filter_map(|i| i.amount_cents).sum();
            let shown = |cents: i64| format!("{:.2}", cents as f64 / 100.0);
            // A table that does not add up to the subtotal is either misread or incomplete.
            if let Some(subtotal) = invoice.subtotal_cents
                && (sum - subtotal).abs() > 1
            {
                invoice.warnings.push(format!(
                    "Line items add up to {} but the subtotal is {}. Check the line items and the subtotal.",
                    shown(sum),
                    shown(subtotal)
                ));
            }
            invoice.items = found;
        }
    }
    let ids: std::collections::HashSet<_> = Regex::new(
        r"(?i)(?:invoice\s*(?:number|no\.?|#)|facture\s*#)\s*:?\s*([A-Z0-9][A-Z0-9_-]*)",
    )
    .unwrap()
    .captures_iter(&text)
    .map(|c| c[1].to_string())
    .filter(|id| id.chars().any(|c| c.is_ascii_digit()))
    .collect();
    if ids.len() > 1 {
        invoice.warnings.push(
            "This PDF contains multiple invoice numbers. Separate its invoices before approval."
                .into(),
        );
    }
    // A supplier a person has already approved is no longer new; every other check still applies.
    if !known && !invoice.remembered_supplier {
        invoice.warnings.push(
            "New supplier layout. Check the extracted fields before adding it to the spreadsheet."
                .into(),
        );
    }
    invoice.warnings.extend(validate(&invoice));
    if !known && matches!(memory, Remembered::Conflict) {
        invoice.warnings.push(
            "This supplier matches more than one name you approved earlier. Confirm the supplier."
                .into(),
        );
    } else if !known && !invoice.remembered_supplier {
        if guess.name.is_none() && !guess.candidates.is_empty() {
            let names: Vec<&str> = guess.candidates.iter().map(|(n, _)| n.as_str()).collect();
            invoice.warnings.push(format!(
                "Confirm the supplier. Possible names: {}.",
                names.join("; ")
            ));
        } else if guess.name.is_some() && !guess.confident {
            invoice
                .warnings
                .push("Confirm the supplier name; it was not clearly identified.".into());
        }
    }
    invoice.warnings.sort();
    invoice.warnings.dedup();
    invoice
}

fn issue_date(text: &str) -> Option<String> {
    let expression = Regex::new(
        r"(?im)(?:Document Date|(?:^|[ \t])Date)\s*:?\s*(\d{1,4}[/-]\d{1,2}[/-]\d{2,4})",
    )
    .unwrap();
    for capture in expression.captures_iter(text) {
        let start = capture.get(0)?.start();
        let prefix = text[..start]
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .trim()
            .to_uppercase();
        if prefix.ends_with("DUE") || prefix.ends_with("SHIP") || prefix.ends_with("SHIPPING") {
            continue;
        }
        return Some(capture[1].into());
    }
    None
}

fn parse_items(text: &str, invoice: &mut Invoice) {
    let fit = Regex::new(r"^\s*\d+\s+([A-Z0-9]+)\s+\d+\s+(.+?)\s+(\d+)\s+(\d+)\s+(\d+)\s+\w+\s+([\d,.]+)\s+([\d,.]+)\s*$").unwrap();
    let bio =
        Regex::new(r"^\s*(.+?)\s+(\d+)\s+([\d ,]+,\d{2})\s+TPS\s+([\d ,]+,\d{2})\s*$").unwrap();
    let ehp = Regex::new(
        r"^\s*(CAN\w+)\s+(.+?)\s+(\d+)\s+\w+\s+\$([\d,.]+)\s+\$([\d,.]+)\s+\d+%\s+\$([\d,.]+)",
    )
    .unwrap();
    let crest = Regex::new(r"^\s*(.*?)\s+(\d+)\s+GST\s+([\d,.]+)\s+([\d,.]+)\s*$").unwrap();
    let mut in_table = false;
    for line in text.lines() {
        if line.contains('\u{c}') {
            in_table = false;
        }
        if line.contains("Item Code")
            || line.contains("ACTIVITY")
            || line.contains("ITEM. NO")
            || line.contains("Item Description")
        {
            in_table = true;
            continue;
        }
        if line.contains("SUBTOTAL")
            || line.contains("Tax Details")
            || line.contains("Total Before Tax")
            || line.contains("Bank Account Details")
            || line.contains("For questions about")
        {
            in_table = false;
        }
        let mut matched = false;
        if invoice.parser.starts_with("fitfoods") {
            if let Some(c) = fit.captures(line) {
                invoice.items.push(LineItem {
                    sku: Some(c[1].into()),
                    description: c[2].trim().into(),
                    quantity: Some(c[4].into()),
                    unit_price_cents: parse_money(&c[6]),
                    amount_cents: parse_money(&c[7]),
                });
                matched = true;
            }
        } else if invoice.parser.starts_with("bioreversa") {
            if let Some(c) = bio.captures(line) {
                invoice.items.push(LineItem {
                    description: c[1].trim().into(),
                    quantity: Some(c[2].into()),
                    unit_price_cents: parse_money(&c[3]),
                    amount_cents: parse_money(&c[4]),
                    ..Default::default()
                });
                matched = true;
            }
        } else if invoice.parser.starts_with("ehplabs") {
            if let Some(c) = ehp.captures(line) {
                invoice.items.push(LineItem {
                    sku: Some(c[1].into()),
                    description: c[2].trim().into(),
                    quantity: Some(c[3].into()),
                    unit_price_cents: parse_money(&c[4]),
                    amount_cents: parse_money(&c[5]),
                });
                matched = true;
            }
        } else if invoice.parser.starts_with("crest")
            && let Some(c) = crest.captures(line)
        {
            invoice.items.push(LineItem {
                description: c[1].trim().into(),
                quantity: Some(c[2].into()),
                unit_price_cents: parse_money(&c[3]),
                amount_cents: parse_money(&c[4]),
                ..Default::default()
            });
            matched = true;
        }
        if in_table
            && !matched
            && !line.trim().is_empty()
            && !line.contains("Gross")
            && !line.trim().eq("Amt")
            && let Some(item) = invoice.items.last_mut()
        {
            item.description.push(' ');
            item.description.push_str(line.trim());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::tests::row;

    /// An invoice laid out the way a hosted invoicing tool does: customer blocks on the left, the
    /// supplier top-right, and a header row of field names above a row of values.
    fn header_row_invoice() -> String {
        [
            row(&[(0, "Invoice")]),
            String::new(),
            row(&[
                (0, "Bill to"),
                (40, "Ship to"),
                (110, "Northern Wick Supply Inc"),
            ]),
            row(&[(0, "Attn: Alex Buyer"), (40, "Attn: Alex Buyer")]),
            row(&[
                (0, "Maple Street Market"),
                (40, "Maple Street Market"),
                (111, "145 Aberdeen Avenue"),
            ]),
            row(&[
                (0, "77 Harbour Road"),
                (40, "77 Harbour Road"),
                (114, "St. John's, NL"),
            ]),
            row(&[
                (0, "Victoria, BC V8V 1A1"),
                (40, "Victoria, BC V8V 1A1"),
                (114, "A1A 5P6"),
            ]),
            row(&[(95, "GST/HST Number: 817712326RT0001")]),
            String::new(),
            row(&[
                (0, "Amount due"),
                (26, "Due date"),
                (46, "Issue date"),
                (66, "Invoice number"),
                (88, "Reference"),
            ]),
            row(&[
                (0, "CAD$613.66"),
                (26, "Oct 29, 2026"),
                (46, "Sep 29, 2026"),
                (66, "INV-5797"),
                (88, "09-29-2026"),
            ]),
            String::new(),
            row(&[
                (1, "Item"),
                (17, "Description"),
                (70, "Quantity"),
                (88, "Price"),
                (100, "Tax"),
                (110, "Amount"),
            ]),
            row(&[
                (1, "WK-1"),
                (17, "Wick Mode Blue"),
                (73, "23"),
                (88, "19.99"),
                (100, "0%"),
                (110, "459.77"),
            ]),
            row(&[
                (1, "WK-2"),
                (17, "Pump Surge"),
                (73, "11"),
                (88, "13.99"),
                (100, "0%"),
                (110, "153.89"),
            ]),
            row(&[(84, "Subtotal"), (110, "613.66")]),
            row(&[(84, "Total sales tax"), (110, "0.00")]),
            row(&[(84, "Total"), (110, "613.66")]),
        ]
        .join("\n")
    }

    #[test]
    fn an_unseen_template_is_read_by_labels_not_by_supplier_specific_code() {
        let context = Context {
            customer: vec!["77 harbour road".into()],
            ..Default::default()
        };
        let result = parse_invoice_with(&header_row_invoice(), 1, &context);
        assert_eq!(result.parser, "generic");
        assert_eq!(result.supplier, "Northern Wick Supply Inc");
        assert_eq!(result.invoice_number, "INV-5797");
        assert_eq!(result.invoice_date.as_deref(), Some("2026-09-29"));
        assert_eq!(result.due_date.as_deref(), Some("2026-10-29"));
        assert_eq!(result.currency.as_deref(), Some("CAD"));
        assert_eq!(result.balance_cents, Some(61366));
        assert_eq!(
            (result.subtotal_cents, result.tax_cents, result.total_cents),
            (Some(61366), Some(0), Some(61366))
        );
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| w.starts_with("Confirm the supplier")),
            "{:?}",
            result.warnings
        );
    }

    fn remembering(entries: &[(&str, &str)]) -> Context {
        Context {
            customer: vec!["77 harbour road".into()],
            known_suppliers: entries
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn an_approved_supplier_is_recognised_by_its_tax_number_or_name_and_is_no_longer_new() {
        let text = header_row_invoice();
        let fresh = parse_invoice_with(&text, 1, &remembering(&[]));
        assert!(
            fresh
                .warnings
                .iter()
                .any(|w| w.starts_with("New supplier layout"))
        );
        assert!(
            fresh
                .identity_keys
                .contains(&"tax:817712326RT0001".to_string())
        );
        assert!(!fresh.remembered_supplier);

        // By tax number, and the person's own spelling of the name is what is used.
        let by_tax = parse_invoice_with(
            &text,
            1,
            &remembering(&[("tax:817712326RT0001", "Northern Wick Supply Inc.")]),
        );
        assert_eq!(by_tax.supplier, "Northern Wick Supply Inc.");
        assert!(by_tax.remembered_supplier);
        assert!(
            by_tax.warnings.is_empty(),
            "everything else checks out, so nothing is held: {:?}",
            by_tax.warnings
        );

        // By name alone, when the document carries no tax number.
        let by_name = parse_invoice_with(
            &text.replace("GST/HST Number: 817712326RT0001", ""),
            1,
            &remembering(&[("name:northernwicksupplyinc", "Northern Wick")]),
        );
        assert_eq!(by_name.supplier, "Northern Wick");
        assert!(by_name.warnings.is_empty(), "{:?}", by_name.warnings);

        // A different supplier is still new.
        let other = parse_invoice_with(
            &text,
            1,
            &remembering(&[("tax:555000111RT0001", "Someone Else Ltd")]),
        );
        assert!(
            other
                .warnings
                .iter()
                .any(|w| w.starts_with("New supplier layout"))
        );
    }

    #[test]
    fn a_remembered_supplier_never_bypasses_the_other_checks() {
        // Same supplier, but this invoice's total does not equal subtotal plus tax.
        let original = header_row_invoice();
        let at = original.rfind("613.66").expect("the total line is last");
        let text = format!(
            "{}700.00{}",
            &original[..at],
            &original[at + "613.66".len()..]
        );
        let result = parse_invoice_with(
            &text,
            1,
            &remembering(&[("tax:817712326RT0001", "Northern Wick Supply Inc")]),
        );
        assert!(result.remembered_supplier);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("does not match the total")),
            "{:?}",
            result.warnings
        );
    }

    #[test]
    fn identifiers_pointing_at_two_confirmed_names_are_held_not_guessed() {
        let result = parse_invoice_with(
            &header_row_invoice(),
            1,
            &remembering(&[
                ("tax:817712326RT0001", "Northern Wick Supply Inc"),
                ("name:northernwicksupplyinc", "Northern Wick Brands Ltd"),
            ]),
        );
        assert!(!result.remembered_supplier);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("more than one name"))
        );
    }

    #[test]
    fn line_items_are_read_for_an_unseen_template_and_must_add_up_to_the_subtotal() {
        let context = Context {
            customer: vec!["77 harbour road".into()],
            ..Default::default()
        };
        let good = parse_invoice_with(&header_row_invoice(), 1, &context);
        assert_eq!(good.items.len(), 2);
        assert_eq!(good.items[0].sku.as_deref(), Some("WK-1"));
        assert_eq!(good.items[1].amount_cents, Some(15389));
        assert!(!good.warnings.iter().any(|w| w.starts_with("Line items")));

        // One line amount is wrong, so the table no longer adds up: held for a person to check.
        let broken = header_row_invoice().replace("153.89", "153.00");
        let result = parse_invoice_with(&broken, 1, &context);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.starts_with("Line items add up to 612.77")),
            "{:?}",
            result.warnings
        );
    }

    #[test]
    fn a_line_item_total_column_is_never_the_invoice_total() {
        let text = [
            row(&[(0, "Quartz Valve Corp.")]),
            row(&[(0, "12 Mill Road")]),
            row(&[(0, "Invoice Number:"), (30, "A-1042")]),
            row(&[
                (0, "Item Code"),
                (14, "Description"),
                (60, "Qty"),
                (70, "Price"),
                (82, "Total"),
            ]),
            row(&[
                (0, "V-1"),
                (14, "Valve"),
                (60, "4"),
                (70, "117.99"),
                (82, "471.96"),
            ]),
            row(&[
                (0, "V-2"),
                (14, "Gasket"),
                (60, "12"),
                (70, "22.50"),
                (82, "270.00"),
            ]),
            row(&[(60, "\u{258c}Total Before Tax:"), (84, "CAD 741.96")]),
            row(&[(60, "Total Tax Amount:"), (84, "CAD 37.10")]),
            row(&[(60, "\u{258c}Total Amount:"), (84, "CAD 779.06")]),
        ]
        .join("\n");
        let result = parse_invoice(&text, 1);
        assert_eq!(
            result.total_cents,
            Some(77906),
            "not the 270.00 under the column heading"
        );
        assert_eq!(result.subtotal_cents, Some(74196));
        assert_eq!(result.tax_cents, Some(3710));
        assert_eq!(result.currency.as_deref(), Some("CAD"));
    }

    #[test]
    fn a_total_that_does_not_reconcile_is_flagged_not_trusted() {
        let text = [
            row(&[(0, "Quartz Valve Corp.")]),
            row(&[(0, "Invoice Number: A-1")]),
            row(&[(60, "Subtotal"), (84, "100.00")]),
            row(&[(60, "Total Tax"), (84, "5.00")]),
            row(&[(60, "Total"), (84, "250.00")]),
        ]
        .join("\n");
        let result = parse_invoice(&text, 1);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("does not match the total")),
            "{:?}",
            result.warnings
        );
    }

    #[test]
    fn one_unambiguous_date_settles_the_convention_for_the_others() {
        // 10/01/2026 alone could be 1 October or 10 January; 10/16/2026 can only be month/day.
        let text = "Acme Foods Ltd\nInvoice Number: A-9\nDate: 10/01/2026\nDue Date: 10/16/2026\nSUBTOTAL 100.00\nTOTAL TAX 5.00\nTOTAL 105.00\nCAD";
        let result = parse_invoice(text, 1);
        assert_eq!(result.invoice_date.as_deref(), Some("2026-10-01"));
        assert_eq!(result.due_date.as_deref(), Some("2026-10-16"));
        // With nothing to settle it, an ambiguous date is still held back.
        let lone = parse_invoice(
            "Acme Foods Ltd\nInvoice Number: A-9\nDate: 10/01/2026\nSUBTOTAL 100.00\nTOTAL TAX 5.00\nTOTAL 105.00\nCAD",
            1,
        );
        assert!(lone.invoice_date.is_none());
    }

    #[test]
    fn a_missing_supplier_is_left_for_review_and_candidates_are_offered() {
        let text = [
            row(&[(0, "Invoice")]),
            row(&[(0, "Invoice Number: Z-77"), (60, "Total"), (84, "10.00")]),
        ]
        .join("\n");
        let result = parse_invoice(&text, 1);
        assert!(
            result.supplier.is_empty(),
            "no guess when nothing identifies a supplier"
        );
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("Confirm the supplier"))
        );
    }
    #[test]
    fn ambiguous_generic_dates_are_not_guessed() {
        let result = parse_invoice(
            "Acme\nInvoice Number: ABC123\nDate: 01/02/2026\nDue Date: 03/04/2026\nSUBTOTAL 100.00\nTOTAL TAX 5.00\nTOTAL 105.00\nCAD",
            1,
        );
        assert!(result.invoice_date.is_none());
        assert!(result.due_date.is_none());
        assert_eq!(result.total_cents, Some(10500));
        assert!(!result.warnings.is_empty());
    }
    #[test]
    fn net_terms_can_disambiguate_dates() {
        let result = parse_invoice(
            "Acme\nInvoice Number: ABC123\nDate: 10/02/2026\nDue Date: 11/01/2026\nNet 30\nSUBTOTAL 100.00\nTOTAL TAX 5.00\nTOTAL 105.00\nCAD",
            1,
        );
        assert_eq!(result.invoice_date.as_deref(), Some("2026-10-02"));
        assert_eq!(result.due_date.as_deref(), Some("2026-11-01"));
    }
}
