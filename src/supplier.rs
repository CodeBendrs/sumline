//! Finds the supplier on an invoice without knowing its template.
//!
//! The name can be anywhere: top-left, top-right, a footer, a "pay to" line. So every plausible
//! name on the page is scored on signals that hold across layouts, and the result is only trusted
//! when one candidate clearly wins:
//!
//! * a legal suffix (Inc, Ltd, LLC, LP ...);
//! * an address, tax registration number, or contact details lined up with the name;
//! * appearing as a "pay to" / "remit to" name;
//! * sitting in a "Bill to / Ship to" block, or containing text the workspace says is its own
//!   (store names and addresses) — those are the customer, never the supplier.
//!
//! Email metadata is deliberately not used: invoices are often forwarded by a person, so the
//! sender says nothing about who issued the document.

use crate::layout::{Layout, is_label_like};
use regex::Regex;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct Guess {
    /// Set when one candidate clearly wins, or when the only plausible candidate is weak but
    /// unopposed (`confident` is then false).
    pub name: Option<String>,
    pub confident: bool,
    /// Best candidates with their scores, strongest first.
    pub candidates: Vec<(String, i32)>,
    /// Tax registration numbers printed beside the chosen name (normalised).
    pub tax_ids: Vec<String>,
}

/// Normalises a name so punctuation and case do not make one supplier look like two.
pub fn name_key(name: &str) -> String {
    format!("name:{}", key(name))
}

/// What identifies this supplier regardless of layout: its tax numbers and its name.
pub fn identity_keys(guess: &Guess) -> Vec<String> {
    let mut keys: Vec<String> = guess.tax_ids.iter().map(|id| format!("tax:{id}")).collect();
    if let Some(name) = &guess.name {
        keys.push(name_key(name));
    }
    keys
}

struct Patterns {
    suffix: Regex,
    street: Regex,
    postal: Regex,
    tax_value: Regex,
    tax_label: Regex,
    recipient_label: Regex,
    recipient_inline: Regex,
    contact: Regex,
    pay_to: Regex,
    sender_inline: Regex,
    sender_label: Regex,
    aka: Regex,
    not_a_name: Regex,
    title: Regex,
    phone: Regex,
    place: Regex,
}

impl Patterns {
    fn new() -> Self {
        let street_words = "street|st|avenue|ave|road|rd|boulevard|blvd|drive|dr|way|lane|ln|court|ct|\
            highway|hwy|crescent|cres|place|pl|parkway|pkwy|terrace|trail|rue|boul|chemin|route|rang|\
            suite|unit|square|sq|circle|cir";
        Self {
            suffix: Regex::new(
                r"(?i)\b(?:inc|incorporated|ltd|limited|llc|l\.l\.c|lp|llp|corp|corporation|co|company|\
                  gmbh|ag|sa|sarl|srl|ulc|plc|pty|bv|nv|lt[ée]e|enr|s\.e\.n\.c|ltda)\b\.?$",
            )
            .unwrap(),
            street: Regex::new(&format!(
                r"(?i)^\s*\d+[a-z]?(?:-[a-z0-9]+)?\s+.*\b(?:{street_words})\b|\b(?:p\.?\s?o\.?\s*box|c\.p\.)\s*\d+|^(?:unit|suite|ste)\.?\s*[\w-]+$"
            ))
            .unwrap(),
            postal: Regex::new(r"(?i)\b[a-z]\d[a-z]\s?\d[a-z]\d\b|\b[A-Z]{2}\s+\d{5}(?:-\d{4})?\b")
                .unwrap(),
            tax_value: Regex::new(
                r"\b\d{9}\s?(?:RT\s?\d{4})?\b|\b\d{10}\s?TQ\s?\d{4}\b|\b\d{2}-\d{7}\b|\b[A-Z]{2}\s?\d{8,12}\b",
            )
            .unwrap(),
            tax_label: Regex::new(
                r"(?i)\b(?:gst|hst|qst|tps|tvq|vat|ein|tax\s*(?:id|no|number)|business\s*(?:no|number)|registration|reg\.?\s*(?:no|number))\b",
            )
            .unwrap(),
            recipient_label: Regex::new(
                r"(?i)^(?:bill(?:ed)?\s*to|ship(?:ped)?\s*to|sold\s*to|deliver(?:ed)?\s*to|invoice\s*to|ship\s*(?:&|and)\s*bill\s*to|customer|client|consignee|buyer|facturer\s*[àa]|livrer\s*[àa]|vendu\s*[àa]|destinataire)\s*:?$",
            )
            .unwrap(),
            recipient_inline: Regex::new(
                r"(?i)^(?:bill(?:ed)?\s*to|ship(?:ped)?\s*to|sold\s*to|deliver(?:ed)?\s*to|invoice\s*to|attn|attention|customer|client)\s*:\s*\S",
            )
            .unwrap(),
            contact: Regex::new(r"(?i)@|\bwww\.|https?://|\.(?:com|ca|net|org)\b|^(?:phone|tel|telephone|fax|toll\s*free|e-?mail|website)\b").unwrap(),
            pay_to: Regex::new(
                r"(?i)(?:make\s+(?:all\s+)?(?:cheques?|checks?)\s+(?:payable\s+)?to|(?:cheques?|checks?)\s+payable\s+to|payable\s+to|remit(?:tance)?\s+to|pay\s+to|e-?transfers?\s+to)\s*:?\s*(.+)$",
            )
            .unwrap(),
            // A label that says "this is the sender". "Ship from" is a warehouse, not the issuer,
            // and a sentence containing "from" is not a label: both are excluded by anchoring.
            sender_inline: Regex::new(
                r"(?i)^(?:from|vendor|supplier|seller|sold\s+by|issued\s+by|billed\s+from|bill\s+from|invoice\s+from)\s*:\s*(\S.*)$",
            )
            .unwrap(),
            sender_label: Regex::new(
                r"(?i)^(?:from|vendor|supplier|seller|sold\s+by|issued\s+by|billed\s+from|bill\s+from|invoice\s+from)\s*:?$",
            )
            .unwrap(),
            aka: Regex::new(
                r"(?i)\s*\((?:formerly|f/k/a|fka|d/b/a|dba|o/a|operating\s+as|a\s+division\s+of)[^)]*\)\s*$",
            )
            .unwrap(),
            not_a_name: Regex::new(
                r"(?i)^(?:phone|tel|telephone|fax|toll\s*free|e-?mail|website|web|gst|hst|qst|vat|tax|attn|attention|page|date|invoice|bill|ship|sold|customer|terms|po|ref|account)\b.*[:#]",
            )
            .unwrap(),
            title: Regex::new(
                r"(?i)^(?:tax\s+invoice|invoice|credit\s+note|statement|receipt|original|duplicate|copy|packing\s+slip|purchase\s+order|sales\s+order|quote|estimate|proforma|remittance|thank\s+you.*|canada|usa|united\s+states|ontario|quebec|british\s+columbia|alberta|manitoba|saskatchewan|nova\s+scotia|new\s+brunswick)$",
            )
            .unwrap(),
            phone: Regex::new(r"^\+?[\d\s().-]{7,}$").unwrap(),
            place: Regex::new(r"(?i)^[a-z .'-]+,\s*[a-z]{2}\b").unwrap(),
        }
    }
}

fn overlap(a: (usize, usize), b: (usize, usize)) -> usize {
    a.1.min(b.1).saturating_sub(a.0.max(b.0))
}

/// Two cells are in the same visual column if they overlap noticeably or start together.
fn aligned(a: (usize, usize), b: (usize, usize)) -> bool {
    let narrow = (a.1 - a.0).min(b.1 - b.0).max(1);
    overlap(a, b) * 10 >= narrow * 4 || a.0.abs_diff(b.0) <= 2
}

fn key(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Cells that belong to the customer: blocks under "Bill to" style labels and anything that
/// contains text the workspace has registered as its own.
fn recipient_cells(
    layout: &Layout,
    customer: &[String],
    patterns: &Patterns,
) -> HashSet<(usize, usize)> {
    let mut found = HashSet::new();
    let mut seeds: Vec<(usize, usize, usize, usize)> = Vec::new(); // line, first col, last col, depth
    for (li, line) in layout.lines.iter().enumerate() {
        for (ci, cell) in line.cells.iter().enumerate() {
            if patterns.recipient_label.is_match(&cell.text) {
                let high = line
                    .cells
                    .get(ci + 1)
                    .map_or(cell.start + 48, |next| next.start);
                seeds.push((li, cell.start.saturating_sub(1), high, 10));
            } else if patterns.recipient_inline.is_match(&cell.text) {
                found.insert((li, ci));
            }
            let lowered = cell.text.to_lowercase();
            let lowered = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
            if customer
                .iter()
                .any(|marker| marker.len() >= 5 && lowered.contains(marker.as_str()))
            {
                found.insert((li, ci));
                // The rest of the customer's block: a few lines either side, same column.
                seeds.push((
                    li.saturating_sub(3),
                    cell.start.saturating_sub(1),
                    cell.start + 48,
                    7,
                ));
            }
        }
    }
    for (first, low, high, depth) in seeds {
        let mut quiet = 0;
        for li in first..(first + depth).min(layout.lines.len()) {
            let line = &layout.lines[li];
            let in_range: Vec<usize> = line
                .cells
                .iter()
                .enumerate()
                .filter(|(_, c)| overlap((c.start, c.end), (low, high)) > 0)
                .map(|(i, _)| i)
                .collect();
            // A table header ends the block.
            if li > first && line.cells.iter().filter(|c| is_label_like(&c.text)).count() >= 3 {
                break;
            }
            if in_range.is_empty() {
                quiet += 1;
                if quiet >= 3 {
                    break;
                }
                continue;
            }
            quiet = 0;
            for ci in in_range {
                found.insert((li, ci));
            }
        }
    }
    found
}

/// A tax registration number, with where its label sits on the page.
struct TaxLine {
    line: usize,
    span: (usize, usize),
    id: String,
}

/// Registration numbers on the page: a number in or just after a label such as "GST/HST".
fn tax_lines(layout: &Layout, patterns: &Patterns) -> Vec<TaxLine> {
    let mut found = Vec::new();
    for (li, line) in layout.lines.iter().enumerate() {
        for cell in &line.cells {
            if !patterns.tax_label.is_match(&cell.text) {
                continue;
            }
            let id = patterns
                .tax_value
                .find(&cell.text)
                .map(|m| m.as_str().to_string())
                .or_else(|| {
                    layout.lines[li + 1..]
                        .iter()
                        .take(2)
                        .flat_map(|l| &l.cells)
                        .find_map(|c| {
                            patterns
                                .tax_value
                                .find(&c.text)
                                .map(|m| m.as_str().to_string())
                        })
                });
            if let Some(id) = id {
                found.push(TaxLine {
                    line: li,
                    span: (cell.start, cell.end),
                    id: id
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>()
                        .to_uppercase(),
                });
            }
        }
    }
    found
}

fn name_of(text: &str, patterns: &Patterns) -> Option<String> {
    let mut name = text.trim().to_string();
    name = patterns.aka.replace(&name, "").trim().to_string();
    let letters = name.chars().filter(|c| c.is_alphabetic()).count();
    let digits = name.chars().filter(|c| c.is_ascii_digit()).count();
    let words: Vec<&str> = name.split_whitespace().collect();
    if name.len() < 3
        || name.len() > 60
        || words.is_empty()
        || words.len() > 8
        || letters < 2
        || digits * 10 > name.chars().count() * 3
        || name.ends_with(':')
        || is_label_like(&name)
        || patterns.title.is_match(&name)
        || patterns.not_a_name.is_match(&name)
        || patterns.contact.is_match(&name)
        || patterns.phone.is_match(&name)
        || patterns.street.is_match(&name)
        || patterns.postal.is_match(&name)
        || patterns.place.is_match(&name)
        || patterns.recipient_inline.is_match(&name)
        || crate::layout::accept_money(&name).is_some()
        || crate::layout::accept_date(&name).is_some()
    {
        return None;
    }
    // Names are written in capitals; sentences mostly are not.
    let small = [
        "of", "and", "the", "de", "la", "du", "des", "&", "for", "y", "et", "dba",
    ];
    let capitalised = words
        .iter()
        .filter(|w| {
            let first = w.chars().next().unwrap_or(' ');
            first.is_uppercase()
                || first.is_ascii_digit()
                || small.contains(&w.to_lowercase().as_str())
        })
        .count();
    if capitalised * 10 < words.len() * 7 {
        return None;
    }
    Some(name)
}

struct Candidate {
    name: String,
    score: i32,
    mentions: usize,
    /// Line and columns of the occurrence that scored best.
    at: (usize, (usize, usize)),
}

pub fn detect(layout: &Layout, customer: &[String]) -> Guess {
    let patterns = Patterns::new();
    let recipient = recipient_cells(layout, customer, &patterns);
    let taxes = tax_lines(layout, &patterns);

    // Values that a "From:", "Vendor", "Sold by" style label points at, inline, to its right, or
    // directly below. These are the strongest evidence there is when a supplier prints them.
    let mut labelled: HashSet<(usize, usize)> = HashSet::new();
    for (li, line) in layout.lines.iter().enumerate() {
        for (ci, cell) in line.cells.iter().enumerate() {
            if patterns.sender_inline.is_match(&cell.text) {
                labelled.insert((li, ci));
            } else if patterns.sender_label.is_match(&cell.text) {
                if line.cells.get(ci + 1).is_some() {
                    labelled.insert((li, ci + 1));
                }
                for below in layout.lines[li + 1..]
                    .iter()
                    .enumerate()
                    .take(2)
                    .filter(|(_, l)| l.page == line.page)
                {
                    for (bi, c) in below.1.cells.iter().enumerate() {
                        if aligned((c.start, c.end), (cell.start, cell.end)) {
                            labelled.insert((li + 1 + below.0, bi));
                        }
                    }
                }
            }
        }
    }

    let last_page = layout.lines.last().map_or(0, |l| l.page);
    let mut seen_on_first_page = 0usize;
    let mut candidates: HashMap<String, Candidate> = HashMap::new();
    let mut first_candidate_taken = false;
    let mut mentions: HashMap<String, usize> = HashMap::new();
    for line in &layout.lines {
        for cell in &line.cells {
            if let Some(name) = name_of(&cell.text, &patterns) {
                *mentions.entry(key(&name)).or_default() += 1;
            }
        }
    }

    for (li, line) in layout.lines.iter().enumerate() {
        if !line.cells.is_empty() && line.page == 0 {
            seen_on_first_page += 1;
        }
        let in_head = line.page == 0 && seen_on_first_page <= 40;
        let in_tail = line.page == last_page && li + 25 >= layout.lines.len();
        for (ci, cell) in line.cells.iter().enumerate() {
            // "Pay to" style lines name the supplier wherever they appear.
            let pay_target = patterns
                .pay_to
                .captures(&cell.text)
                .and_then(|c| name_of(c[1].trim(), &patterns));
            let label_target = if labelled.contains(&(li, ci)) {
                let value = patterns
                    .sender_inline
                    .captures(&cell.text)
                    .map_or(cell.text.as_str(), |c| c.get(1).map_or("", |m| m.as_str()));
                name_of(value.trim(), &patterns)
            } else {
                None
            };
            let (name, base) = match (label_target, pay_target) {
                (Some(name), _) => (name, 5),
                (None, Some(name)) => (name, 4),
                (None, None) if in_head || in_tail => match name_of(&cell.text, &patterns) {
                    Some(name) => (name, 0),
                    None => continue,
                },
                (None, None) => continue,
            };
            let span = (cell.start, cell.end);
            let mut score = base;
            if patterns.suffix.is_match(&name) {
                score += 3;
            }
            if line.page == 0 {
                score += match seen_on_first_page {
                    0..=4 => 2,
                    5..=15 => 1,
                    _ => 0,
                };
            }
            // Something that looks like an address under the name, in the same column.
            let mut street = false;
            let mut postal = false;
            for below in layout.lines[li + 1..]
                .iter()
                .take(5)
                .filter(|l| l.page == line.page)
            {
                for c in below
                    .cells
                    .iter()
                    .filter(|c| aligned((c.start, c.end), span))
                {
                    street |= patterns.street.is_match(&c.text);
                    postal |= patterns.postal.is_match(&c.text) || patterns.place.is_match(&c.text);
                }
            }
            if street {
                score += 3;
            } else if postal {
                score += 2;
            }
            if taxes
                .iter()
                .any(|t| t.line.abs_diff(li) <= 8 && aligned(t.span, span))
            {
                score += 3;
            }
            let contact = layout.lines[li.saturating_sub(2)..(li + 8).min(layout.lines.len())]
                .iter()
                .flat_map(|l| &l.cells)
                .any(|c| aligned((c.start, c.end), span) && patterns.contact.is_match(&c.text));
            if contact {
                score += 1;
            }
            if recipient.contains(&(li, ci)) {
                score -= 9;
            }
            // The classic place for the issuer's name, as a mild tie-breaker.
            if line.page == 0 && !first_candidate_taken && base == 0 {
                first_candidate_taken = true;
                score += 1;
            }
            if mentions.get(&key(&name)).copied().unwrap_or(0) >= 2 {
                score += 1;
            }
            let entry = candidates.entry(key(&name)).or_insert(Candidate {
                name,
                score: i32::MIN,
                mentions: 0,
                at: (li, span),
            });
            entry.mentions += 1;
            if score > entry.score {
                entry.score = score;
                entry.at = (li, span);
            }
        }
    }

    let mut ranked: Vec<Candidate> = candidates.into_values().collect();
    ranked.sort_by(|a, b| b.score.cmp(&a.score).then(a.name.cmp(&b.name)));
    let listing: Vec<(String, i32)> = ranked
        .iter()
        .filter(|c| c.score > 0)
        .take(4)
        .map(|c| (c.name.clone(), c.score))
        .collect();
    let top = ranked.first();
    let second = ranked.get(1).map_or(i32::MIN, |c| c.score);
    let (name, confident) = match top {
        Some(top) if top.score >= 7 && top.score.saturating_sub(second) >= 3 => {
            (Some(top.name.clone()), true)
        }
        Some(top) if top.score >= 4 && second < 2 => (Some(top.name.clone()), false),
        _ => (None, false),
    };
    let tax_ids = match (&name, top) {
        (Some(_), Some(top)) => {
            let mut ids: Vec<String> = taxes
                .iter()
                .filter(|t| t.line.abs_diff(top.at.0) <= 8 && aligned(t.span, top.at.1))
                .map(|t| t.id.clone())
                .collect();
            ids.sort();
            ids.dedup();
            ids
        }
        _ => Vec::new(),
    };
    Guess {
        name,
        confident,
        candidates: listing,
        tax_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::tests::row;

    fn guess(lines: &[String], customer: &[&str]) -> Guess {
        let customer: Vec<String> = customer.iter().map(|c| c.to_lowercase()).collect();
        detect(&Layout::new(&lines.join("\n")), &customer)
    }

    /// The customer block every test invoice carries, at a given column.
    fn customer_block(column: usize) -> Vec<String> {
        vec![
            row(&[(column, "Bill to")]),
            row(&[(column, "Attn: Alex Buyer")]),
            row(&[(column, "Maple Street Market")]),
            row(&[(column, "77 Harbour Road")]),
            row(&[(column, "Victoria, BC V8V 1A1")]),
        ]
    }

    #[test]
    fn supplier_in_the_top_right_beside_a_bill_to_block() {
        let mut lines = vec![row(&[(0, "Invoice")]), String::new()];
        lines.push(row(&[
            (0, "Bill to"),
            (40, "Ship to"),
            (100, "Northern Wick Supply Inc"),
        ]));
        lines.push(row(&[(0, "Attn: Alex Buyer"), (40, "Attn: Alex Buyer")]));
        lines.push(row(&[
            (0, "Maple Street Market"),
            (40, "Maple Street Market"),
            (101, "145 Aberdeen Avenue"),
        ]));
        lines.push(row(&[
            (0, "77 Harbour Road"),
            (40, "77 Harbour Road"),
            (104, "St. John's, NL"),
        ]));
        lines.push(row(&[
            (0, "Victoria, BC V8V 1A1"),
            (40, "Victoria, BC V8V 1A1"),
            (104, "A1A 5P6"),
        ]));
        lines.push(row(&[(95, "GST/HST Number: 817712326RT0001")]));
        let result = guess(&lines, &[]);
        assert_eq!(
            result.name.as_deref(),
            Some("Northern Wick Supply Inc"),
            "{:?}",
            result.candidates
        );
        assert!(result.confident);
    }

    #[test]
    fn the_winning_names_tax_number_is_reported_and_the_customers_is_not() {
        let mut lines = vec![
            row(&[(0, "Maple Valve Works Ltd"), (70, "Bill to")]),
            row(&[(0, "9 Foundry Road"), (70, "Corner Market Inc")]),
            row(&[
                (0, "Burnaby, BC V5C 2K1"),
                (70, "GST/HST Number: 999888777RT0001"),
            ]),
            row(&[(0, "GST/HST No.: 123456789 RT0001")]),
        ];
        lines.push(row(&[(0, "Invoice 55")]));
        let result = guess(
            &lines,
            &["corner market inc".into()]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            result.name.as_deref(),
            Some("Maple Valve Works Ltd"),
            "{:?}",
            result.candidates
        );
        assert_eq!(
            result.tax_ids,
            ["123456789RT0001"],
            "not the customer's 999888777RT0001"
        );
        assert_eq!(
            identity_keys(&result),
            ["tax:123456789RT0001", "name:maplevalveworksltd"]
        );
    }

    #[test]
    fn supplier_top_left_without_a_legal_suffix_is_found_by_address_and_tax_number() {
        let mut lines = vec![
            row(&[(0, "BioHarvest")]),
            row(&[(0, "2005 Rue Charbonneau")]),
            row(&[(0, "Trois-Rivières QC G9A 5C9")]),
            row(&[(0, "info@bioharvest.example")]),
            row(&[(0, "GST/HST Registration No.:")]),
            row(&[(0, "772818290RT0001")]),
            String::new(),
            row(&[(0, "INVOICE")]),
        ];
        lines.extend(customer_block(0));
        let result = guess(&lines, &[]);
        assert_eq!(
            result.name.as_deref(),
            Some("BioHarvest"),
            "{:?}",
            result.candidates
        );
        assert!(result.confident);
    }

    #[test]
    fn supplier_in_the_middle_of_the_page_after_noise() {
        let mut lines = vec![
            row(&[(110, "Invoice Number: A-7788")]),
            String::new(),
            row(&[(110, "Sales Order")]),
            String::new(),
            row(&[(0, "Pacific Labs International LLC")]),
            row(&[(0, "Scarborough, ON, M1N 3J1, Canada")]),
            row(&[(0, "Company Registration Number: 732712310 RT0001")]),
            String::new(),
        ];
        lines.extend(customer_block(0));
        let result = guess(&lines, &[]);
        assert_eq!(
            result.name.as_deref(),
            Some("Pacific Labs International LLC"),
            "{:?}",
            result.candidates
        );
    }

    #[test]
    fn the_customer_is_never_chosen_even_when_it_looks_like_a_company() {
        // Only the customer's block carries an address and a legal suffix; nothing else does.
        let lines = vec![
            row(&[(0, "Invoice")]),
            row(&[(0, "Maple Street Market Ltd")]),
            row(&[(0, "77 Harbour Road")]),
            row(&[(0, "Victoria, BC V8V 1A1")]),
        ];
        let result = guess(&lines, &["77 harbour road"]);
        assert_ne!(result.name.as_deref(), Some("Maple Street Market Ltd"));
        assert!(result.name.is_none() || !result.confident, "{:?}", result);
    }

    #[test]
    fn a_pay_to_line_in_the_footer_identifies_the_supplier() {
        let mut lines = vec![row(&[(0, "Invoice 4410")])];
        lines.extend(customer_block(0));
        lines.extend((0..30).map(|_| String::new()));
        lines.push(row(&[(
            0,
            "Please make cheques payable to: Harbour Fire Protection Ltd.",
        )]));
        let result = guess(&lines, &[]);
        assert_eq!(
            result.name.as_deref(),
            Some("Harbour Fire Protection Ltd."),
            "{:?}",
            result.candidates
        );
    }

    #[test]
    fn an_explicit_sender_label_wins_wherever_the_name_sits() {
        // Inline "From:" deep in the page, with a competing letterhead-looking customer above.
        let mut lines = vec![row(&[(0, "Invoice 8812")])];
        lines.extend(customer_block(0));
        lines.extend((0..12).map(|_| String::new()));
        lines.push(row(&[(0, "From: Delta Cable Works Ltd")]));
        assert_eq!(
            guess(&lines, &[]).name.as_deref(),
            Some("Delta Cable Works Ltd")
        );

        // A label in one cell with the name in the cell beside it.
        let mut lines = vec![row(&[(0, "Invoice 8812")])];
        lines.extend((0..12).map(|_| String::new()));
        lines.push(row(&[(0, "Vendor"), (20, "Delta Cable Works Ltd")]));
        assert_eq!(
            guess(&lines, &[]).name.as_deref(),
            Some("Delta Cable Works Ltd")
        );

        // A label above the name.
        let mut lines = vec![row(&[(0, "Invoice 8812")])];
        lines.extend((0..12).map(|_| String::new()));
        lines.push(row(&[(60, "Sold by")]));
        lines.push(row(&[(60, "Delta Cable Works Ltd")]));
        assert_eq!(
            guess(&lines, &[]).name.as_deref(),
            Some("Delta Cable Works Ltd")
        );
    }

    #[test]
    fn ship_from_and_ordinary_sentences_are_not_sender_labels() {
        let lines = vec![
            row(&[(0, "Invoice 8812")]),
            row(&[(0, "Ship from: Rear Warehouse Annex")]),
            row(&[(
                0,
                "Claims must be made within 24 hours from Receipt Of Goods",
            )]),
        ];
        let result = guess(&lines, &[]);
        assert!(
            result.name.as_deref() != Some("Rear Warehouse Annex")
                && result.name.as_deref() != Some("Receipt Of Goods"),
            "{:?}",
            result
        );
    }

    #[test]
    fn nothing_plausible_means_no_name_rather_than_a_guess() {
        let lines = vec![
            row(&[(0, "Invoice")]),
            row(&[(0, "Date: 2026-10-01"), (50, "Total 15.00")]),
            row(&[(0, "1 x Item"), (50, "15.00")]),
        ];
        assert!(guess(&lines, &[]).name.is_none());
    }

    #[test]
    fn a_formerly_known_as_note_is_not_part_of_the_name() {
        let lines = vec![
            row(&[(0, "FIT FOODS LP (formerly Fit Foods Ltd.)")]),
            row(&[(0, "1589 Kebet Way")]),
            row(&[(0, "Port Coquitlam BC V3C 6L5")]),
        ];
        assert_eq!(guess(&lines, &[]).name.as_deref(), Some("FIT FOODS LP"));
    }
}
