//! Layout-aware reading of extracted invoice text.
//!
//! Poppler's layout mode keeps the horizontal position of text. This module turns each line into
//! cells with column positions, so a value can be found next to, below, or inside its label no
//! matter how a supplier arranged the page. Nothing here knows any supplier's template.

use chrono::NaiveDate;
use regex::Regex;

#[derive(Debug, Clone)]
pub struct Cell {
    pub text: String,
    /// First and one-past-last character column.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Line {
    pub page: usize,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub lines: Vec<Line>,
}

/// Removes characters that PDF generators insert around values and that break matching.
pub fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !matches!(
                c,
                '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}' | '\u{ad}'
            )
        })
        // Non-breaking spaces, and the bars and blocks some generators draw beside headings.
        .map(|c| {
            if c == '\u{a0}' || ('\u{2580}'..='\u{259f}').contains(&c) || c == '\u{2502}' {
                ' '
            } else {
                c
            }
        })
        .filter(|c| *c != '\r')
        .collect()
}

fn split_cells(raw: &str) -> Vec<Cell> {
    let chars: Vec<char> = raw.chars().collect();
    let mut cells = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let start = i;
        let mut last = i;
        while i < chars.len() {
            if chars[i].is_whitespace() {
                let mut j = i;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                // Two or more spaces (or a tab) separate columns; one space is inside a phrase.
                if j - i >= 2 || chars[i] == '\t' || j >= chars.len() {
                    break;
                }
                i = j;
            } else {
                last = i;
                i += 1;
            }
        }
        cells.push(Cell {
            text: chars[start..=last].iter().collect(),
            start,
            end: last + 1,
        });
    }
    cells
}

impl Layout {
    pub fn new(text: &str) -> Self {
        let mut lines = Vec::new();
        for (page, chunk) in clean(text).split('\u{c}').enumerate() {
            for raw in chunk.lines() {
                lines.push(Line {
                    page,
                    cells: split_cells(raw),
                });
            }
        }
        Self { lines }
    }
}

/// Words that name a field rather than hold its value. A cell made only of these is a label.
const LABEL_WORDS: &[&str] = &[
    "invoice",
    "inv",
    "facture",
    "date",
    "due",
    "issue",
    "issued",
    "amount",
    "total",
    "subtotal",
    "sub",
    "reference",
    "ref",
    "po",
    "p",
    "o",
    "number",
    "no",
    "terms",
    "customer",
    "description",
    "quantity",
    "qty",
    "price",
    "tax",
    "balance",
    "currency",
    "bill",
    "billed",
    "ship",
    "shipped",
    "attn",
    "attention",
    "page",
    "item",
    "rate",
    "unit",
    "account",
    "order",
    "sales",
    "rep",
    "via",
    "tracking",
    "to",
    "of",
    "please",
    "pay",
    "payment",
    "document",
    "taxe",
    "montant",
    "echeance",
    "net",
    "gst",
    "hst",
    "pst",
    "qst",
    "vat",
    "from",
    "id",
    "cad",
    "usd",
    "and",
];

pub fn is_label_like(text: &str) -> bool {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect();
    !words.is_empty()
        && words.len() <= 4
        && words.iter().all(|w| LABEL_WORDS.contains(&w.as_str()))
        && !text.chars().any(|c| c.is_ascii_digit())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// "Invoice number: 123" in one cell.
    Inline,
    /// The label is a cell and the value is the cell to its right.
    Right,
    /// A header row above a row of values.
    Below,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub value: String,
    pub line: usize,
    pub how: How,
}

fn overlap(a: (usize, usize), b: (usize, usize)) -> usize {
    a.1.min(b.1).saturating_sub(a.0.max(b.0))
}

/// Finds every value attached to a label, in document order.
///
/// `labels` is a regex alternation of label phrases (without anchors). A cell counts as a label
/// when it consists of nothing else, apart from a trailing colon or a currency note. A cell that
/// starts with the label and continues is read as "label: value" and kept only if the rest is
/// accepted. `accept` turns a candidate cell into a value or rejects it.
///
/// With `rightmost`, the last accepted cell to the right wins; totals are right-aligned and may
/// have a currency or percentage column between the label and the amount.
pub fn find_labelled(
    layout: &Layout,
    labels: &str,
    rightmost: bool,
    accept: &dyn Fn(&str) -> Option<String>,
) -> Vec<Hit> {
    let pure = Regex::new(&format!(
        r"(?i)^(?:{labels})\s*(?:\((?:[a-z$]{{1,4}})\))?\s*[:#.]*\s*$"
    ))
    .unwrap();
    let inline = Regex::new(&format!(
        r"(?i)^(?:{labels})\s*[:#.]+\s*(.+)$|^(?:{labels})\s+(\S.*)$"
    ))
    .unwrap();
    let mut hits = Vec::new();
    for (li, line) in layout.lines.iter().enumerate() {
        for (ci, cell) in line.cells.iter().enumerate() {
            if let Some(found) = inline.captures(&cell.text) {
                let rest = found.get(1).or_else(|| found.get(2)).map(|m| m.as_str());
                if let Some(value) = rest.and_then(|r| accept(r.trim())) {
                    hits.push(Hit {
                        value,
                        line: li,
                        how: How::Inline,
                    });
                    continue;
                }
            }
            if !pure.is_match(&cell.text) {
                continue;
            }
            let to_the_right = &line.cells[ci + 1..];
            let picked = if rightmost {
                to_the_right.iter().rev().find_map(|c| accept(&c.text))
            } else {
                // The first value before the next label; a neighbouring label means the values
                // are in the row below, not here.
                to_the_right
                    .iter()
                    .map(|c| (accept(&c.text), is_label_like(&c.text)))
                    .take_while(|(value, label)| value.is_some() || !*label)
                    .find_map(|(value, _)| value)
            };
            if let Some(value) = picked {
                hits.push(Hit {
                    value,
                    line: li,
                    how: How::Right,
                });
                continue;
            }
            // A header row over a row of values: same column, or same position in the row.
            let next_start = line.cells.get(ci + 1).map_or(cell.end + 12, |c| c.start);
            let span = (cell.start, next_start.max(cell.end));
            let mut seen = 0;
            for below in layout.lines[li + 1..].iter().take(3) {
                if below.page != line.page {
                    break;
                }
                if below.cells.is_empty() {
                    continue;
                }
                seen += 1;
                let by_index = (below.cells.len() == line.cells.len()).then(|| &below.cells[ci]);
                let by_position = below
                    .cells
                    .iter()
                    .filter(|c| overlap((c.start, c.end), span) > 0)
                    .max_by_key(|c| overlap((c.start, c.end), span));
                if let Some(value) = by_index
                    .and_then(|c| accept(&c.text))
                    .or_else(|| by_position.and_then(|c| accept(&c.text)))
                {
                    hits.push(Hit {
                        value,
                        line: li,
                        how: How::Below,
                    });
                    break;
                }
                if seen >= 2 {
                    break;
                }
            }
        }
    }
    hits
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn month_number(name: &str) -> Option<u32> {
    let short = name.get(..3)?.to_lowercase();
    MONTHS
        .iter()
        .position(|m| *m == short)
        .map(|i| i as u32 + 1)
}

/// Parses "Oct 29, 2026", "29 October 2026", "Fri, Oct 2nd 2026" and similar.
pub fn named_date(raw: &str) -> Option<NaiveDate> {
    let month = r"(jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|may|june?|july?|aug(?:ust)?|sep(?:t(?:ember)?)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?)\.?";
    let month_first = Regex::new(&format!(
        r"(?i)^(?:[a-z]{{3,9}},?\s+)?{month}\s+(\d{{1,2}})(?:st|nd|rd|th)?,?\s+(\d{{4}})$"
    ))
    .unwrap();
    let day_first = Regex::new(&format!(
        r"(?i)^(?:[a-z]{{3,9}},?\s+)?(\d{{1,2}})(?:st|nd|rd|th)?\s+{month},?\s+(\d{{4}})$"
    ))
    .unwrap();
    let text = raw.trim();
    let (m, d, y) = if let Some(c) = month_first.captures(text) {
        (c[1].to_string(), c[2].to_string(), c[3].to_string())
    } else {
        let c = day_first.captures(text)?;
        (c[2].to_string(), c[1].to_string(), c[3].to_string())
    };
    NaiveDate::from_ymd_opt(y.parse().ok()?, month_number(&m)?, d.parse().ok()?)
}

/// Accepts a cell that is entirely a date, in numeric or written-out form.
pub fn accept_date(cell: &str) -> Option<String> {
    let text = cell.trim().trim_end_matches([',', '.']);
    let numeric =
        Regex::new(r"^\d{4}[/.-]\d{1,2}[/.-]\d{1,2}$|^\d{1,2}[/.-]\d{1,2}[/.-]\d{2,4}$").unwrap();
    if numeric.is_match(text) || named_date(text).is_some() {
        Some(text.to_string())
    } else {
        None
    }
}

/// Accepts an identifier-like value: has a digit, is short, and is not a date or an amount.
pub fn accept_identifier(cell: &str) -> Option<String> {
    let text = cell.trim().trim_end_matches([',', ';']);
    let shape = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/-]{1,31}$").unwrap();
    if shape.is_match(text)
        && text.chars().any(|c| c.is_ascii_digit())
        && accept_date(text).is_none()
        && !Regex::new(r"^\d[\d,]*\.\d{2}$").unwrap().is_match(text)
        && !is_label_like(text)
    {
        Some(text.to_string())
    } else {
        None
    }
}

/// Accepts a free-text reference such as a purchase-order note: short, not a label, date or
/// amount. Looser than an identifier because suppliers print whatever the buyer typed.
pub fn accept_reference(cell: &str) -> Option<String> {
    let text = cell.trim().trim_end_matches([',', ';']);
    if (2..=40).contains(&text.chars().count())
        && text.chars().any(|c| c.is_alphanumeric())
        && !is_label_like(text)
        && accept_date(text).is_none()
        && accept_money(text).is_none()
    {
        Some(text.to_string())
    } else {
        None
    }
}

/// Accepts an amount, with or without currency decoration ("CAD$1,609.86", "$ 12.00 CAD").
pub fn accept_money(cell: &str) -> Option<String> {
    let text = cell.trim();
    let shape = Regex::new(
        r"(?i)^(?:CAD|USD|CDN|CA|US|C|U)?\s*\$?\s*(-?\(?\d[\d ,.]*[.,]\d{2}\)?)\s*(?:CAD|USD)?$",
    )
    .unwrap();
    let amount = shape.captures(text)?.get(1)?.as_str().to_string();
    crate::model::parse_money(&amount).map(|_| amount)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Builds a line with each piece of text starting at its column, like Poppler's layout mode.
    pub fn row(pieces: &[(usize, &str)]) -> String {
        let mut line = String::new();
        for (column, text) in pieces {
            while line.chars().count() < *column {
                line.push(' ');
            }
            line.push_str(text);
        }
        line
    }

    #[test]
    fn cells_keep_their_columns_and_single_spaces_stay_inside_a_cell() {
        let line = row(&[(0, "Bill to"), (39, "Ship to"), (80, "Purchase order 12")]);
        let cells = split_cells(&line);
        let texts: Vec<_> = cells.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["Bill to", "Ship to", "Purchase order 12"]);
        assert_eq!((cells[1].start, cells[1].end), (39, 46));
    }

    #[test]
    fn zero_width_characters_do_not_hide_values() {
        let layout = Layout::new("Invoice number   \u{200b}INV-5797\n");
        assert_eq!(layout.lines[0].cells[1].text, "INV-5797");
    }

    #[test]
    fn header_row_over_value_row_is_read_by_column_not_by_neighbour() {
        let text = [
            row(&[
                (0, "Amount due"),
                (28, "Due date"),
                (50, "Issue date"),
                (71, "Invoice number"),
                (93, "Reference"),
            ]),
            row(&[
                (0, "CAD$1,609.86"),
                (28, "Oct 29, 2026"),
                (50, "Sep 29, 2026"),
                (71, "INV-5797"),
                (93, "09-29-2026"),
            ]),
        ]
        .join("\n");
        let layout = Layout::new(&text);
        let number = find_labelled(&layout, "invoice number", false, &accept_identifier);
        assert_eq!(
            number[0].value, "INV-5797",
            "not the neighbouring 'Reference' label"
        );
        assert_eq!(number[0].how, How::Below);
        let due = find_labelled(&layout, "due date", false, &accept_date);
        assert_eq!(due[0].value, "Oct 29, 2026");
        let amount = find_labelled(&layout, "amount due", false, &accept_money);
        assert_eq!(amount[0].value, "1,609.86");
    }

    #[test]
    fn inline_and_adjacent_labels_work_and_totals_take_the_rightmost_amount() {
        let text = [
            row(&[(50, "Invoice No: A-1042")]),
            row(&[(50, "Due Date:"), (70, "2026-11-01")]),
            row(&[(50, "Total"), (75, "CAD"), (85, "1,234.50")]),
            row(&[(50, "Total sales tax"), (85, "0.00")]),
        ]
        .join("\n");
        let layout = Layout::new(&text);
        assert_eq!(
            find_labelled(&layout, "invoice no", false, &accept_identifier)[0].value,
            "A-1042"
        );
        assert_eq!(
            find_labelled(&layout, "due date", false, &accept_date)[0].value,
            "2026-11-01"
        );
        let totals = find_labelled(&layout, "total", true, &accept_money);
        assert_eq!(totals.len(), 1, "'Total sales tax' is a different label");
        assert_eq!(totals[0].value, "1,234.50");
    }

    #[test]
    fn written_out_dates_parse() {
        for (raw, expected) in [
            ("Oct 29, 2026", (2026, 10, 29)),
            ("29 October 2026", (2026, 10, 29)),
            ("Fri, Oct 2nd 2026", (2026, 10, 2)),
            ("Sept. 5, 2026", (2026, 9, 5)),
        ] {
            let date = named_date(raw).unwrap_or_else(|| panic!("{raw}"));
            assert_eq!((date.year(), date.month(), date.day()), expected, "{raw}");
        }
        assert!(named_date("Octember 5, 2026").is_none());
        assert!(accept_date("see attached").is_none());
    }

    use chrono::Datelike;
}
