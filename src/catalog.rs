//! Bounded, encrypted query results. Invoice money and supplier names stay encrypted.
use crate::{
    model::Record,
    storage::{Store, hash},
};
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Filters {
    pub supplier: String,
    pub status: String,
    pub from: String,
    pub to: String,
    pub sort: String,
    pub currency: String,
}
impl Store {
    pub fn filtered_page(
        &self,
        account: &str,
        view: &str,
        selected: &str,
        search: &str,
        page: usize,
        filters: &Filters,
    ) -> Result<Value> {
        let key = hash(&serde_json::to_vec(&(
            view, selected, search, page, filters,
        ))?);
        let db = self.db.lock().unwrap();
        if let Some(bytes) = db
            .query_row(
                "SELECT response FROM query_cache WHERE account=?1 AND key=?2",
                params![account, key],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()?
        {
            return self.read_json(account, &bytes);
        }
        let mut stmt=db.prepare("SELECT record,revision FROM invoices WHERE account=?1 ORDER BY created_at DESC,id DESC")?;
        let rows = stmt.query_map([account], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut records = Vec::new();
        let mut suppliers = BTreeMap::<String, String>::new();
        let mut currencies = BTreeMap::new();
        let mut global_counts = BTreeMap::<String, usize>::new();
        for row in rows {
            let (bytes, revision) = row?;
            let mut record: Record = self.read_json(account, &bytes)?;
            record.revision = revision;
            *global_counts.entry(record.status.clone()).or_default() += 1;
            if !record.invoice.supplier.is_empty() {
                suppliers
                    .entry(record.invoice.supplier.trim().to_lowercase())
                    .or_insert(record.invoice.supplier.clone());
            }
            if let Some(currency) = &record.invoice.currency {
                currencies.insert(currency.clone(), true);
            }
            record.invoice.items.clear();
            records.push(record);
        }
        let needle = search.to_lowercase();
        records.retain(|r| matches(r, selected, &needle, filters));
        let mut totals = BTreeMap::<String, (i64, i64, usize)>::new();
        let mut counts = BTreeMap::<String, usize>::new();
        for record in &records {
            *counts.entry(record.status.clone()).or_default() += 1;
            if ["ready", "approved"].contains(&record.status.as_str())
                && record.store_id.is_some()
                && let Some(currency) = &record.invoice.currency
            {
                let total = totals.entry(currency.clone()).or_default();
                total.0 = total
                    .0
                    .saturating_add(record.invoice.total_cents.unwrap_or(0));
                total.1 = total
                    .1
                    .saturating_add(record.invoice.tax_cents.unwrap_or(0));
                total.2 += 1;
            }
        }
        records.retain(|r| {
            (view == "archive"
                || (view == "review" && r.status == "review")
                || (view == "invoices" && !["excluded", "bundle"].contains(&r.status.as_str())))
                && (filters.status.is_empty()
                    || r.status == filters.status
                    || (filters.status == "posted"
                        && ["ready", "approved"].contains(&r.status.as_str())))
        });
        sort_records(&mut records, filters);
        let total = records.len();
        let records: Vec<_> = records
            .into_iter()
            .skip(page.saturating_sub(1).saturating_mul(50))
            .take(50)
            .collect();
        let currency_options: Vec<_> = currencies.into_keys().collect();
        let currencies:Vec<_>=totals.into_iter().map(|(currency,(total_cents,tax_cents,invoice_count))|json!({"currency":currency,"total_cents":total_cents,"tax_cents":tax_cents,"invoice_count":invoice_count})).collect();
        let result = json!({"invoices":records,"total":total,"page":page,"page_size":50,"counts":counts,"global_counts":global_counts,"suppliers":suppliers.into_values().collect::<Vec<_>>(),"currencies":currencies,"currency_options":currency_options});
        db.execute(
            "INSERT INTO query_cache VALUES (?1,?2,?3,?4)",
            params![
                account,
                key,
                self.seal_json(account, &result)?,
                chrono::Utc::now().timestamp_millis()
            ],
        )?;
        db.execute("DELETE FROM query_cache WHERE account=?1 AND key NOT IN (SELECT key FROM query_cache WHERE account=?1 ORDER BY created_at DESC,key LIMIT 128)",[account])?;
        Ok(result)
    }
}

pub fn matches(r: &Record, selected: &str, needle: &str, filters: &Filters) -> bool {
    let date = r.invoice.invoice_date.as_deref();
    (filters.status.is_empty()
        || r.status == filters.status
        || (filters.status == "posted" && ["ready", "approved"].contains(&r.status.as_str())))
        && (selected.is_empty()
            || (selected == "unassigned" && r.store_id.is_none())
            || r.store_id.as_deref() == Some(selected))
        && (filters.supplier.is_empty()
            || r.invoice
                .supplier
                .trim()
                .eq_ignore_ascii_case(filters.supplier.trim()))
        && (filters.currency.is_empty()
            || r.invoice.currency.as_deref() == Some(filters.currency.as_str()))
        && (filters.from.is_empty() || date.is_some_and(|d| d >= filters.from.as_str()))
        && (filters.to.is_empty() || date.is_some_and(|d| d <= filters.to.as_str()))
        && (needle.is_empty()
            || format!(
                "{} {} {} {}",
                r.invoice.supplier,
                r.invoice.invoice_number,
                r.file_name,
                r.invoice.po_number.as_deref().unwrap_or("")
            )
            .to_lowercase()
            .contains(needle))
}
pub fn sort_records(records: &mut [Record], filters: &Filters) {
    records.sort_by(|a, b| {
        use std::cmp::Ordering;
        let order = match filters.sort.as_str() {
            "invoice_oldest" | "invoice_newest" => {
                match (&a.invoice.invoice_date, &b.invoice.invoice_date) {
                    (Some(a), Some(b)) => {
                        if filters.sort == "invoice_oldest" {
                            a.cmp(b)
                        } else {
                            b.cmp(a)
                        }
                    }
                    (None, Some(_)) => Ordering::Greater,
                    (Some(_), None) => Ordering::Less,
                    _ => Ordering::Equal,
                }
            }
            "supplier_az" => a
                .invoice
                .supplier
                .to_lowercase()
                .cmp(&b.invoice.supplier.to_lowercase()),
            "total_high" | "total_low" => {
                a.invoice.currency.cmp(&b.invoice.currency).then_with(|| {
                    match (a.invoice.total_cents, b.invoice.total_cents) {
                        (Some(a), Some(b)) => {
                            if filters.sort == "total_low" {
                                a.cmp(&b)
                            } else {
                                b.cmp(&a)
                            }
                        }
                        (None, Some(_)) => Ordering::Greater,
                        (Some(_), None) => Ordering::Less,
                        _ => Ordering::Equal,
                    }
                })
            }
            _ => b.imported_at.cmp(&a.imported_at),
        };
        order.then_with(|| b.id.cmp(&a.id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn totals_use_invoice_dates_respect_filters_and_never_mix_currencies() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store.db.lock().unwrap().execute_batch("INSERT INTO workspaces VALUES ('a','A');INSERT INTO stores VALUES ('s1','a','First','[]'),('s2','a','Second','[]');").unwrap();
        let data = [
            ("2026-09-30", "CAD", 10000, "Acme", "s1", "ready"),
            ("2026-10-01", "CAD", 20000, "Acme", "s1", "approved"),
            ("2026-10-31", "USD", 30000, "Acme", "s1", "ready"),
            ("2026-11-01", "CAD", 40000, "Acme", "s1", "ready"),
            ("2026-10-02", "CAD", 50000, "Other", "s2", "ready"),
            ("2026-10-03", "CAD", 60000, "Acme", "s1", "review"),
        ];
        for (index, (date, currency, total, supplier, branch, status)) in
            data.into_iter().enumerate()
        {
            let id = store
                .insert_document(
                    "a",
                    "invoice.pdf",
                    format!("%PDF-{index}").as_bytes(),
                    "Upload",
                    None,
                    None,
                )
                .unwrap()
                .unwrap();
            let mut record = store.record("a", &id).unwrap().unwrap();
            record.status = status.into();
            record.store_id = Some(branch.into());
            record.invoice.supplier = supplier.into();
            record.invoice.invoice_number = index.to_string();
            record.invoice.invoice_date = Some(date.into());
            record.invoice.currency = Some(currency.into());
            record.invoice.total_cents = Some(total);
            record.invoice.tax_cents = Some(100);
            store.commit_record("a", &record, None).unwrap();
        }
        let filters = Filters {
            supplier: "acme".into(),
            from: "2026-10-01".into(),
            to: "2026-10-31".into(),
            sort: "invoice_oldest".into(),
            ..Filters::default()
        };
        let page = store
            .filtered_page("a", "invoices", "s1", "", 1, &filters)
            .unwrap();
        assert_eq!(page["total"], 3);
        let totals = page["currencies"].as_array().unwrap();
        assert_eq!(totals.len(), 2);
        assert_eq!(totals[0]["total_cents"], 20000);
        assert_eq!(totals[1]["total_cents"], 30000);
        assert_eq!(page["invoices"][0]["invoice"]["invoice_date"], "2026-10-01");
        let repeat = store
            .filtered_page("a", "invoices", "s1", "", 1, &filters)
            .unwrap();
        assert_eq!(page, repeat);
        let currency = Filters {
            currency: "USD".into(),
            ..filters.clone()
        };
        assert_eq!(
            store
                .filtered_page("a", "invoices", "s1", "", 1, &currency)
                .unwrap()["total"],
            1
        );
        let review = Filters {
            status: "review".into(),
            ..filters.clone()
        };
        assert!(
            store
                .filtered_page("a", "invoices", "s1", "", 1, &review)
                .unwrap()["currencies"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let workbook = crate::spreadsheet::filtered_workbook(
            &store,
            "a",
            Some("s1"),
            &currency,
            "",
            "http://localhost:3030",
        )
        .unwrap();
        assert!(workbook.starts_with(b"PK"));
        let mut record = store
            .records("a")
            .unwrap()
            .into_iter()
            .find(|r| r.invoice.currency.as_deref() == Some("USD"))
            .unwrap();
        record.status = "excluded".into();
        store.commit_record("a", &record, None).unwrap();
        let changed = store
            .filtered_page("a", "invoices", "s1", "", 1, &filters)
            .unwrap();
        assert_eq!(changed["currencies"].as_array().unwrap().len(), 1);
    }
}
