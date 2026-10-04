//! Reads the line-item table of an invoice without knowing the supplier's template.
//!
//! The table is found by its heading row (several recognised column names), each cell below is
//! assigned to the nearest heading, and a row is a line that carries an amount. Text lines
//! between rows are wrapped descriptions and are attached to the row they belong to.
//!
//! What it deliberately does not do: guess. If no heading row is found, or a heading is
//! ambiguous, nothing is returned; the caller also checks that the items add up to the subtotal.

use crate::layout::{Cell, Layout, accept_money};
use crate::model::{LineItem, parse_money};
use regex::Regex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Sku,
    /// "Item" alone: the product code when a description column exists, otherwise the name.
    Item,
    Description,
    /// Quantity columns, with a priority: billed quantities beat ordered ones.
    Qty(u8),
    Price,
    Amount,
    Other,
}

struct Heading {
    kind: Kind,
    start: usize,
    end: usize,
}

struct Patterns {
    sku: Regex,
    item: Regex,
    description: Regex,
    shipped: Regex,
    quantity: Regex,
    ordered: Regex,
    units: Regex,
    price: Regex,
    amount: Regex,
    modifier: Regex,
    total_label: Regex,
}

impl Patterns {
    fn new() -> Self {
        let r = |p: &str| Regex::new(&format!("(?i)^(?:{p})$")).unwrap();
        Self {
            sku: r(r"sku|item\s*(?:code|no\.?|#|number)|item\.\s*no\.?|code|part\s*(?:no\.?|#|number)|product\s*code|article\s*(?:no\.?|#)"),
            item: r(r"items?"),
            description: r(r"description|desc\.?|details?|particulars|products?|services?|activity|designation|désignation|libellé|article|items?\s+description"),
            shipped: r(r"shipped|shp"),
            quantity: r(r"quantity|qty\.?|qté|quantité"),
            ordered: r(r"ordered|ord\.?"),
            units: r(r"units?|hours?|hrs?"),
            price: r(r"unit\s*price|price|rate|unit\s*cost|prix(?:\s*unitaire)?|tarif|cost|each"),
            amount: r(r"amount|amt\.?|total|line\s*total|extended|ext\.?(?:\s*(?:price|amount|amt))?|montant|net\s*amount|line\s*amount"),
            modifier: r(r"gross|net|unit|line|extended|ext\.?|sub"),
            total_label: Regex::new(
                r"(?i)^(?:sub-?\s?total(?:\s*\(?[a-z]{3}\)?)?|total(?:\s+(?:before\s+tax|tax|amount|due|invoice))?|total\s*\(?(?:cad|usd)\)?|balance(?:\s+due)?|amount\s+due|grand\s+total|total\s+due|please\s+pay|tax\s+details|bank\s+account\s+details)\s*:?$",
            )
            .unwrap(),
        }
    }

    fn classify(&self, text: &str) -> Kind {
        let text = text.trim().trim_end_matches([':', '.']).trim();
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.to_lowercase().starts_with("gross") {
            return Kind::Other;
        }
        if self.sku.is_match(&text) {
            Kind::Sku
        } else if self.item.is_match(&text) {
            Kind::Item
        } else if self.description.is_match(&text) {
            Kind::Description
        } else if self.shipped.is_match(&text) {
            Kind::Qty(3)
        } else if self.quantity.is_match(&text) {
            Kind::Qty(2)
        } else if self.ordered.is_match(&text) {
            Kind::Qty(1)
        } else if self.units.is_match(&text) {
            Kind::Qty(0)
        } else if self.price.is_match(&text) {
            Kind::Price
        } else if self.amount.is_match(&text) {
            Kind::Amount
        } else {
            Kind::Other
        }
    }
}

fn distance(a: (usize, usize), b: (usize, usize)) -> usize {
    if a.1.min(b.1) > a.0.max(b.0) {
        0
    } else {
        a.0.saturating_sub(b.1).max(b.0.saturating_sub(a.1))
    }
}

/// Splits a heading cell holding several headings ("QTY DESCRIPTION") using exact columns.
fn headings_of(cell: &Cell, above: Option<&[Cell]>, p: &Patterns) -> Vec<Heading> {
    let mut text = cell.text.clone();
    // A modifier printed on the line above ("Gross" over "Amt") belongs to this heading.
    if let Some(cells) = above {
        for c in cells {
            if p.modifier.is_match(&c.text)
                && distance((c.start, c.end), (cell.start, cell.end)) <= 2
            {
                text = format!("{} {}", c.text, cell.text);
            }
        }
    }
    let whole = p.classify(&text);
    if whole != Kind::Other {
        return vec![Heading {
            kind: whole,
            start: cell.start,
            end: cell.end,
        }];
    }
    let mut parts = Vec::new();
    let mut offset = 0;
    for word in cell.text.split(' ') {
        let start = cell.start + cell.text[..offset].chars().count();
        let end = start + word.chars().count();
        parts.push(Heading {
            kind: p.classify(word),
            start,
            end,
        });
        offset += word.len() + 1;
    }
    if parts.iter().filter(|h| h.kind != Kind::Other).count() >= 2 {
        parts.retain(|h| h.kind != Kind::Other);
        return parts;
    }
    vec![Heading {
        kind: Kind::Other,
        start: cell.start,
        end: cell.end,
    }]
}

fn heading_row(layout: &Layout, li: usize, p: &Patterns) -> Option<Vec<Heading>> {
    let line = &layout.lines[li];
    if line.cells.len() < 2 {
        return None;
    }
    let above = li
        .checked_sub(1)
        .map(|i| &layout.lines[i])
        .filter(|l| l.page == line.page)
        .map(|l| l.cells.as_slice());
    let headings: Vec<Heading> = line
        .cells
        .iter()
        .flat_map(|c| headings_of(c, above, p))
        .collect();
    let named = headings.iter().filter(|h| h.kind != Kind::Other).count();
    let has_name = headings
        .iter()
        .any(|h| matches!(h.kind, Kind::Description | Kind::Item | Kind::Sku));
    let has_amount = headings.iter().any(|h| h.kind == Kind::Amount);
    (named >= 3 && has_name && has_amount).then_some(headings)
}

/// One printed row plus the text lines that wrap around it.
#[derive(Default)]
struct Draft {
    sku: Vec<(usize, String)>,
    description: Vec<String>,
    sku_as_text: Vec<String>,
    quantity: Option<String>,
    price: Option<i64>,
    amount: Option<i64>,
}

/// Column meanings and positions for the current page. Poppler lays each page out on its own
/// scale, so the heading row is read again whenever it repeats.
struct Columns {
    kinds: Vec<Kind>,
    spans: Vec<(usize, usize)>,
}

fn columns(mut headings: Vec<Heading>) -> Columns {
    // "Item" is the product code when a description column exists, and the name otherwise.
    let has_description = headings.iter().any(|h| h.kind == Kind::Description);
    for h in &mut headings {
        if h.kind == Kind::Item {
            h.kind = if has_description {
                Kind::Sku
            } else {
                Kind::Description
            };
        }
    }
    let best_qty = headings
        .iter()
        .filter_map(|h| match h.kind {
            Kind::Qty(priority) => Some(priority),
            _ => None,
        })
        .max();
    // When a price or amount heading repeats, the leftmost is the net figure: totals that include
    // tax or shipping come after it.
    let mut seen_price = false;
    let mut seen_amount = false;
    let kinds = headings
        .iter()
        .map(|h| match (h.kind, best_qty) {
            (Kind::Qty(priority), Some(best)) if priority != best => Kind::Other,
            (Kind::Price, _) if std::mem::replace(&mut seen_price, true) => Kind::Other,
            (Kind::Amount, _) if std::mem::replace(&mut seen_amount, true) => Kind::Other,
            (kind, _) => kind,
        })
        .collect();
    Columns {
        kinds,
        spans: headings.iter().map(|h| (h.start, h.end)).collect(),
    }
}

fn assign(spans: &[(usize, usize)], cell: &Cell) -> Option<usize> {
    spans
        .iter()
        .enumerate()
        .map(|(i, s)| (i, distance((cell.start, cell.end), *s)))
        .filter(|(_, d)| *d <= 8)
        .min_by_key(|(_, d)| *d)
        .map(|(i, _)| i)
}

/// A leading line number printed in the code column ("12 62511CA") is not part of the code.
fn without_row_number(code: &str) -> String {
    let numbered = Regex::new(r"^\d{1,3}\s+([A-Za-z0-9][A-Za-z0-9._/-]{3,})$").unwrap();
    numbered
        .captures(code.trim())
        .map_or_else(|| code.trim().to_string(), |c| c[1].to_string())
}

pub fn parse(layout: &Layout) -> Vec<LineItem> {
    let p = Patterns::new();
    let Some((header_line, headings)) =
        (0..layout.lines.len()).find_map(|li| heading_row(layout, li, &p).map(|h| (li, h)))
    else {
        return Vec::new();
    };
    let mut cols = columns(headings);

    let mut blocks: Vec<Vec<Draft>> = Vec::new();
    let mut current: Vec<(bool, Draft)> = Vec::new();
    let flush = |current: &mut Vec<(bool, Draft)>, blocks: &mut Vec<Vec<Draft>>| {
        if current.iter().any(|(anchor, _)| *anchor) {
            blocks.push(attach(std::mem::take(current)));
        } else {
            current.clear();
        }
    };
    let mut blank_run = 0;
    let mut page = layout.lines[header_line].page;
    for (offset, line) in layout.lines[header_line + 1..].iter().enumerate() {
        let li = header_line + 1 + offset;
        if line.page != page {
            page = line.page;
            blank_run = 0;
            // A page break ends the rows above it; text at the top of the next page is not theirs.
            flush(&mut current, &mut blocks);
        }
        if line.cells.is_empty() {
            blank_run += 1;
            flush(&mut current, &mut blocks);
            if blank_run >= 4 {
                break;
            }
            continue;
        }
        blank_run = 0;
        let repeated_heading = line
            .cells
            .iter()
            .flat_map(|c| headings_of(c, None, &p))
            .filter(|h| h.kind != Kind::Other)
            .count()
            >= 3;
        if repeated_heading {
            flush(&mut current, &mut blocks);
            if let Some(refreshed) = heading_row(layout, li, &p) {
                cols = columns(refreshed);
            }
            continue;
        }
        if line.cells.iter().any(|c| p.total_label.is_match(&c.text)) {
            break;
        }
        let mut draft = Draft::default();
        let mut best_sku: Option<(usize, &Cell)> = None;
        for cell in &line.cells {
            let Some(column) = assign(&cols.spans, cell) else {
                continue;
            };
            match cols.kinds[column] {
                Kind::Description => draft.description.push(cell.text.clone()),
                Kind::Sku => {
                    draft.sku_as_text.push(cell.text.clone());
                    let overlap = distance((cell.start, cell.end), cols.spans[column]);
                    if best_sku.is_none_or(|(d, _)| overlap < d) {
                        best_sku = Some((overlap, cell));
                    }
                }
                Kind::Qty(_) => draft.quantity = Some(cell.text.clone()),
                Kind::Price => {
                    draft.price = accept_money(&cell.text).and_then(|v| parse_money(&v));
                }
                Kind::Amount => {
                    draft.amount = accept_money(&cell.text).and_then(|v| parse_money(&v));
                }
                _ => {}
            }
        }
        if let Some((_, cell)) = best_sku {
            draft.sku.push((cell.start, cell.text.clone()));
        }
        current.push((draft.amount.is_some(), draft));
    }
    flush(&mut current, &mut blocks);

    let mut items: Vec<LineItem> = blocks
        .into_iter()
        .flatten()
        .filter(|d| d.amount.is_some())
        .map(|d| LineItem {
            description: d.description.join(" ").trim().to_string(),
            sku: d.sku.first().map(|(_, t)| without_row_number(t)),
            quantity: d.quantity,
            unit_price_cents: d.price,
            amount_cents: d.amount,
        })
        .collect();
    // Some layouts put the description under a heading that says "item number". If no row has
    // a description but they all have text there, that text is the description.
    let missing_text = items.iter().all(|i| i.description.is_empty());
    if missing_text {
        for item in &mut items {
            item.description = item.sku.take().unwrap_or_default();
        }
    }
    items
}

/// Turns the lines of one block into items. Wrapped text before the first row, or around a row
/// that has no description of its own, belongs to that row; other text continues the row above.
fn attach(lines: Vec<(bool, Draft)>) -> Vec<Draft> {
    let mut items: Vec<Draft> = Vec::new();
    let mut pending_prefix: Vec<String> = Vec::new();
    let mut pending_sku_text: Vec<String> = Vec::new();
    for (is_anchor, mut draft) in lines {
        if is_anchor {
            let mut description = std::mem::take(&mut pending_prefix);
            description.append(&mut draft.description);
            draft.description = description;
            let mut sku_text = std::mem::take(&mut pending_sku_text);
            sku_text.append(&mut draft.sku_as_text);
            draft.sku_as_text = sku_text;
            items.push(draft);
        } else if let Some(last) = items.last_mut() {
            last.description.append(&mut draft.description);
            last.sku_as_text.append(&mut draft.sku_as_text);
        } else {
            pending_prefix.append(&mut draft.description);
            pending_sku_text.append(&mut draft.sku_as_text);
        }
    }
    // A description that is only a number is a quantity that sat under the wrong word.
    let numeric = Regex::new(r"^\d+(?:[.,]\d+)?$").unwrap();
    for item in &mut items {
        let text = item.description.join(" ");
        if item.quantity.is_none() && numeric.is_match(text.trim()) {
            item.quantity = Some(text.trim().to_string());
            item.description.clear();
        }
    }
    // With the "item number" fallback, wrapped text sits in the code column.
    for item in &mut items {
        if item.description.is_empty() && !item.sku_as_text.is_empty() {
            item.sku.clear();
            item.description = vec![item.sku_as_text.join(" ")];
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::tests::row;

    fn items(lines: &[String]) -> Vec<LineItem> {
        parse(&Layout::new(&lines.join("\n")))
    }
    fn amounts(items: &[LineItem]) -> Vec<i64> {
        items.iter().filter_map(|i| i.amount_cents).collect()
    }

    #[test]
    fn rows_with_a_description_wrapped_above_and_below() {
        let found = items(&[
            row(&[
                (1, "Item"),
                (17, "Description"),
                (68, "Quantity"),
                (84, "Price"),
                (96, "Tax"),
                (106, "Amount"),
            ]),
            String::new(),
            row(&[
                (1, "WK-286"),
                (17, "Wick Mode Blue Raz"),
                (70, "23"),
                (84, "19.99"),
                (96, "0%"),
                (108, "459.77"),
            ]),
            String::new(),
            row(&[(17, "Wick Mode Osaka Cherry")]),
            row(&[
                (1, "WK-288"),
                (70, "23"),
                (84, "19.99"),
                (96, "0%"),
                (108, "459.77"),
            ]),
            row(&[(17, "Blossom]")]),
            String::new(),
            row(&[
                (17, "Sales tax - BC 5%"),
                (70, "1"),
                (84, "76.66"),
                (96, "0%"),
                (108, "76.66"),
            ]),
            String::new(),
            row(&[(84, "Subtotal"), (106, "996.20")]),
            row(&[(84, "Total"), (106, "996.20")]),
        ]);
        assert_eq!(amounts(&found), [45977, 45977, 7666]);
        assert_eq!(found[0].sku.as_deref(), Some("WK-286"));
        assert_eq!(found[1].description, "Wick Mode Osaka Cherry Blossom]");
        assert_eq!(found[1].quantity.as_deref(), Some("23"));
        assert_eq!(found[1].unit_price_cents, Some(1999));
        assert_eq!(found[2].description, "Sales tax - BC 5%");
    }

    #[test]
    fn dense_rows_with_continuation_lines_and_decimal_commas() {
        let found = items(&[
            row(&[
                (1, "ACTIVITY"),
                (45, "QTY"),
                (60, "RATE"),
                (72, "TAXE"),
                (86, "AMOUNT"),
            ]),
            row(&[
                (1, "5lbs Isolate Caramel"),
                (46, "4"),
                (60, "96,99"),
                (72, "TPS"),
                (86, "387,96"),
            ]),
            row(&[
                (1, "5lbs Isolate Double Choc"),
                (46, "2"),
                (60, "0,00"),
                (72, "TPS"),
                (90, "0,00"),
            ]),
            row(&[(1, "Free - remplacement")]),
            row(&[
                (1, "5lbs Isolate Maple Brown"),
                (46, "6"),
                (60, "96,99"),
                (72, "TPS"),
                (86, "581,94"),
            ]),
            row(&[(1, "Sugar")]),
            String::new(),
            String::new(),
            row(&[(40, "SUBTOTAL"), (86, "969,90")]),
        ]);
        assert_eq!(amounts(&found), [38796, 0, 58194]);
        assert_eq!(
            found[1].description,
            "5lbs Isolate Double Choc Free - remplacement"
        );
        assert_eq!(found[2].description, "5lbs Isolate Maple Brown Sugar");
        assert_eq!(found[0].unit_price_cents, Some(9699));
    }

    #[test]
    fn a_heading_that_says_item_number_over_the_description_is_repaired() {
        // "QTY DESCRIPTION" is one cell and the description actually sits in the first column.
        let found = items(&[
            row(&[
                (1, "ITEM. NO"),
                (14, "QTY DESCRIPTION"),
                (40, "TAX"),
                (52, "UNIT PRICE"),
                (70, "AMOUNT"),
            ]),
            String::new(),
            row(&[
                (1, "Portable"),
                (15, "1"),
                (40, "GST"),
                (54, "155.00"),
                (72, "155.00"),
            ]),
            row(&[(1, "Inspections/Emerg.")]),
            row(&[(1, "Lights")]),
            String::new(),
            row(&[(50, "SUBTOTAL"), (72, "155.00")]),
        ]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].description, "Portable Inspections/Emerg. Lights");
        assert!(found[0].sku.is_none());
        assert_eq!(found[0].quantity.as_deref(), Some("1"));
        assert_eq!(found[0].amount_cents, Some(15500));
    }

    #[test]
    fn gross_amount_is_not_the_line_amount_and_unit_of_measure_is_not_the_quantity() {
        let found = items(&[
            row(&[(112, "Gross")]),
            row(&[
                (1, "Item"),
                (16, "Description"),
                (62, "Quantity"),
                (73, "Units"),
                (82, "Rate"),
                (92, "Amount"),
                (104, "GST"),
                (112, "Amt"),
            ]),
            String::new(),
            row(&[
                (1, "CAN-1047"),
                (16, "Thermogenic Apple 60srv"),
                (66, "36"),
                (73, "Tub"),
                (82, "$22.00"),
                (92, "$792.00"),
                (104, "5%"),
                (112, "$831.60"),
            ]),
            String::new(),
            row(&[(20, "SUB-TOTAL(CAD)"), (92, "$792.00")]),
        ]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].amount_cents, Some(79200), "not the gross 831.60");
        assert_eq!(found[0].unit_price_cents, Some(2200));
        assert_eq!(found[0].quantity.as_deref(), Some("36"), "not 'Tub'");
        assert_eq!(found[0].sku.as_deref(), Some("CAN-1047"));

        // Even if the "Gross" word is missing, the leftmost amount column is the line amount.
        let unmodified = items(&[
            row(&[
                (1, "Item"),
                (16, "Description"),
                (62, "Quantity"),
                (82, "Rate"),
                (92, "Amount"),
                (104, "GST"),
                (112, "Amt"),
            ]),
            row(&[
                (1, "CAN-1047"),
                (16, "Thermogenic Apple"),
                (66, "36"),
                (82, "$22.00"),
                (92, "$792.00"),
                (104, "5%"),
                (112, "$831.60"),
            ]),
            String::new(),
            row(&[(20, "SUBTOTAL"), (92, "$792.00")]),
        ]);
        assert_eq!(amounts(&unmodified), [79200]);
    }

    #[test]
    fn shipped_beats_ordered_row_numbers_are_ignored_and_a_repeated_heading_continues_the_table() {
        let heading = row(&[
            (2, "Item Code"),
            (14, "Whs"),
            (20, "Description"),
            (62, "Ordered"),
            (72, "Shipped"),
            (82, "B.O."),
            (90, "UoM"),
            (100, "Price"),
            (112, "Total"),
        ]);
        let first = row(&[
            (0, "1"),
            (4, "21152EX"),
            (14, "10"),
            (20, "WHEY Triple Chocolate 4.54 kg"),
            (66, "5"),
            (76, "4"),
            (84, "1"),
            (90, "EA"),
            (100, "117.99"),
            (112, "471.96"),
        ]);
        let second = row(&[
            (0, "2"),
            (4, "51001CA"),
            (14, "10"),
            (20, "PLANT-PRO Vanilla 840 g"),
            (66, "12"),
            (76, "12"),
            (84, "0"),
            (90, "EA"),
            (100, "22.50"),
            (112, "270.00"),
        ]);
        let text = format!(
            "{}\n{first}\n\u{c}{}\n{second}\n\n{}\n",
            heading,
            heading,
            row(&[(100, "Total Before Tax:"), (118, "CAD 741.96")])
        );
        let found = parse(&Layout::new(&text));
        assert_eq!(amounts(&found), [47196, 27000]);
        assert_eq!(
            found[0].quantity.as_deref(),
            Some("4"),
            "shipped, not ordered"
        );
        assert_eq!(
            found[0].sku.as_deref(),
            Some("21152EX"),
            "not the row number"
        );
        assert_eq!(found[1].description, "PLANT-PRO Vanilla 840 g");
    }

    #[test]
    fn each_page_may_lay_the_table_out_on_its_own_scale() {
        // Page two is the same table with every column in a different place, as Poppler produces
        // for some documents. Rows there must still be read, and the text at the top of the page
        // (a repeated letterhead) must not be glued onto the last row of page one.
        let page_one = [
            row(&[
                (2, "Item Code"),
                (14, "Description"),
                (60, "Shipped"),
                (72, "Price"),
                (84, "Total"),
            ]),
            String::new(),
            row(&[
                (0, "1 AB-1001"),
                (14, "Valve body"),
                (62, "4"),
                (72, "10.00"),
                (84, "40.00"),
            ]),
            row(&[(90, "Original"), (110, "INVOICE")]),
        ]
        .join("\n");
        let page_two = [
            row(&[(90, "Document Number"), (120, "Page")]),
            row(&[
                (2, "Item Code"),
                (19, "Description"),
                (88, "Shipped"),
                (101, "Price"),
                (115, "Total"),
            ]),
            String::new(),
            row(&[
                (0, "2 AB-1002"),
                (19, "Gasket ring"),
                (90, "12"),
                (101, "22.50"),
                (115, "270.00"),
            ]),
            String::new(),
            row(&[(100, "Total Before Tax:"), (120, "CAD 310.00")]),
        ]
        .join("\n");
        let found = parse(&Layout::new(&format!("{page_one}\n\u{c}{page_two}")));
        assert_eq!(amounts(&found), [4000, 27000]);
        assert_eq!(
            found[0].description, "Valve body",
            "no letterhead text appended"
        );
        assert_eq!(
            found[0].sku.as_deref(),
            Some("AB-1001"),
            "row number removed"
        );
        assert_eq!(found[1].quantity.as_deref(), Some("12"));
    }

    #[test]
    fn a_quantity_printed_under_the_description_word_is_still_the_quantity() {
        // "QTY DESCRIPTION" is one cell, and the number sits under the second word.
        let found = items(&[
            row(&[
                (1, "ITEM. NO"),
                (28, "QTY DESCRIPTION"),
                (73, "TAX"),
                (85, "UNIT PRICE"),
                (105, "AMOUNT"),
            ]),
            String::new(),
            row(&[
                (1, "Portable"),
                (33, "1"),
                (72, "GST"),
                (88, "155.00"),
                (105, "155.00"),
            ]),
            row(&[(1, "Lights")]),
            String::new(),
            row(&[(60, "SUBTOTAL"), (105, "155.00")]),
        ]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].quantity.as_deref(), Some("1"));
        assert_eq!(found[0].description, "Portable Lights");
        assert!(found[0].sku.is_none());
    }

    #[test]
    fn no_heading_row_means_no_items() {
        assert!(
            items(&[
                row(&[(0, "Consulting services for September")]),
                row(&[(60, "Total"), (80, "1,200.00")]),
            ])
            .is_empty()
        );
    }
}
