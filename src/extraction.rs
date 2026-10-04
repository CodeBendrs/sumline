use crate::model::{Invoice, LineItem, parse_money};
use chrono::NaiveDate;
use regex::Regex;

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
    let text = text.replace('\u{a0}', " ").replace('\r', "");
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
    let is_ehp = text.contains("EHPLabs");
    let is_bio = text.contains("BioReversa");
    let is_fit = text.contains("FIT FOODS");
    let is_crest = text.contains("Crest Fire");
    invoice.supplier = if is_ehp {
        "EHPLabs International LLC".into()
    } else if is_bio {
        "BioReversa".into()
    } else if is_fit {
        "FIT FOODS LP".into()
    } else if is_crest {
        "Crest Fire Extinguishers Ltd.".into()
    } else {
        text.lines()
            .find(|s| !s.trim().is_empty() && !s.to_lowercase().contains("invoice"))
            .unwrap_or("")
            .trim()
            .to_string()
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
    parse_items(&text, &mut invoice);
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
    if !is_ehp && !is_bio && !is_fit && !is_crest {
        invoice.warnings.push(
            "New supplier layout. Check the extracted fields before adding it to the spreadsheet."
                .into(),
        );
    }
    invoice.warnings.extend(validate(&invoice));
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
