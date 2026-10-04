use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LineItem {
    pub description: String,
    pub sku: Option<String>,
    pub quantity: Option<String>,
    pub unit_price_cents: Option<i64>,
    pub amount_cents: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Invoice {
    pub supplier: String,
    pub invoice_number: String,
    pub invoice_date: Option<String>,
    pub due_date: Option<String>,
    pub original_invoice_date: String,
    pub original_due_date: String,
    pub currency: Option<String>,
    pub subtotal_cents: Option<i64>,
    pub tax_cents: Option<i64>,
    pub total_cents: Option<i64>,
    pub balance_cents: Option<i64>,
    pub po_number: Option<String>,
    pub items: Vec<LineItem>,
    pub warnings: Vec<String>,
    pub parser: String,
    pub pages: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    #[serde(default)]
    pub revision: i64,
    pub store_id: Option<String>,
    #[serde(default)]
    pub original_document_id: Option<String>,
    pub file_name: String,
    pub status: String,
    pub imported_at: String,
    pub source: String,
    pub email_subject: Option<String>,
    pub email_sender: Option<String>,
    pub invoice: Invoice,
}

pub fn parse_money(raw: &str) -> Option<i64> {
    let mut value: String = raw
        .chars()
        .filter(|c| c.is_ascii_digit() || ".,-()".contains(*c))
        .collect();
    if value.is_empty() {
        return None;
    }
    let negative = value.starts_with('-') || (value.starts_with('(') && value.ends_with(')'));
    value = value.replace(['-', '(', ')'], "");
    let decimal = value
        .rfind(['.', ','])
        .filter(|&i| value.len() - i - 1 == 2);
    let (whole, fraction) = if let Some(index) = decimal {
        (
            value[..index].replace(['.', ','], ""),
            value[index + 1..].to_string(),
        )
    } else {
        if value.contains('.') {
            return None;
        }
        (value.replace(',', ""), "00".to_string())
    };
    let cents = whole
        .parse::<i64>()
        .ok()?
        .checked_mul(100)?
        .checked_add(fraction.parse::<i64>().ok()?)?;
    if cents > 10_000_000_000_000 {
        return None;
    }
    Some(if negative { -cents } else { cents })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn international_amounts_use_integer_cents() {
        assert_eq!(parse_money("CAD $11,034.72"), Some(1103472));
        assert_eq!(parse_money("2 036,80"), Some(203680));
        assert_eq!(parse_money("(162.75)"), Some(-16275));
        assert_eq!(parse_money("008103"), Some(810300));
        assert_eq!(parse_money("not a price"), None);
        assert_eq!(parse_money("155.125"), None);
    }
}
