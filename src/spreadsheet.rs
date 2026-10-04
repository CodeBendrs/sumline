use crate::{model::Record, storage::Store};
use anyhow::Result;
use rusqlite::OptionalExtension;
use rust_xlsxwriter::{
    Color, ExcelDateTime, Format, Table, TableColumn, TableStyle, Url, Workbook,
};
use serde_json::Value;

pub fn build_workbook(records: &[Record], stores: &[Value], origin: &str) -> Result<Vec<u8>> {
    let mut workbook = Workbook::new();
    let header = Format::new()
        .set_bold()
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x225447));
    let cash = Format::new().set_num_format("#,##0.00;-#,##0.00");
    let date_format = Format::new().set_num_format("d mmm yyyy");
    let sheet = workbook.add_worksheet();
    sheet.set_name("Invoices")?;
    let labels = [
        "Supplier",
        "Invoice number",
        "Invoice date",
        "Due date",
        "Currency",
        "Subtotal",
        "Tax",
        "Total",
        "Invoice balance",
        "PO number",
        "Source",
        "Imported at",
        "Record ID",
        "Store",
        "Original PDF",
    ];
    for (col, label) in labels.iter().enumerate() {
        sheet.write_string_with_format(0, col as u16, *label, &header)?;
    }
    sheet.set_freeze_panes(1, 0)?;
    sheet.set_column_width(0, 36)?;
    sheet.set_column_width(1, 24)?;
    sheet.set_column_range_width(2, 3, 16)?;
    sheet.set_column_width(4, 12)?;
    sheet.set_column_range_width(5, 8, 18)?;
    sheet.set_column_width(9, 22)?;
    sheet.set_column_width(10, 16)?;
    sheet.set_column_width(11, 27)?;
    sheet.set_column_width(12, 40)?;
    let published: Vec<_> = records
        .iter()
        .filter(|r| (r.status == "ready" || r.status == "approved") && r.store_id.is_some())
        .collect();
    for (index, record) in published.iter().enumerate() {
        let row = (index + 1) as u32;
        let i = &record.invoice;
        for (col, value) in [
            (0, i.supplier.as_str()),
            (1, i.invoice_number.as_str()),
            (4, i.currency.as_deref().unwrap_or("")),
            (9, i.po_number.as_deref().unwrap_or("")),
            (10, record.source.as_str()),
            (11, record.imported_at.as_str()),
            (12, record.id.as_str()),
        ] {
            // write_string deliberately never interprets invoice text as a formula or hyperlink.
            sheet.write_string(row, col, value)?;
        }
        for (col, value) in [(2, i.invoice_date.as_deref()), (3, i.due_date.as_deref())] {
            if let Some(value) = value {
                let date = ExcelDateTime::parse_from_str(value)?;
                sheet.write_datetime_with_format(row, col, &date, &date_format)?;
            }
        }
        let store_name = stores
            .iter()
            .find(|s| s["id"].as_str() == record.store_id.as_deref())
            .and_then(|s| s["name"].as_str())
            .unwrap_or("Unassigned");
        sheet.write_string(row, 13, store_name)?;
        sheet.write_url(
            row,
            14,
            Url::new(format!("{origin}/api/invoices/{}/document", record.id)).set_text("Open PDF"),
        )?;
        for (col, value) in [
            (5, i.subtotal_cents),
            (6, i.tax_cents),
            (7, i.total_cents),
            (8, i.balance_cents),
        ] {
            if let Some(value) = value {
                sheet.write_number_with_format(row, col, value as f64 / 100.0, &cash)?;
            }
        }
    }
    sheet.set_column_width(13, 40)?;
    sheet.set_column_width(14, 18)?;
    if !published.is_empty() {
        sheet.add_table(
            0,
            0,
            published.len() as u32,
            14,
            &Table::new().set_style(TableStyle::None).set_columns(
                &labels
                    .iter()
                    .map(|s| TableColumn::new().set_header(*s).set_header_format(&header))
                    .collect::<Vec<_>>(),
            ),
        )?;
    }
    let items = workbook.add_worksheet();
    items.set_name("Line items")?;
    let labels = [
        "Invoice record ID",
        "Invoice number",
        "SKU",
        "Description",
        "Quantity",
        "Unit price",
        "Line amount",
    ];
    for (col, label) in labels.iter().enumerate() {
        items.write_string_with_format(0, col as u16, *label, &header)?;
    }
    items.set_freeze_panes(1, 0)?;
    items.set_column_width(0, 40)?;
    items.set_column_range_width(1, 2, 24)?;
    items.set_column_width(3, 65)?;
    items.set_column_range_width(4, 6, 18)?;
    let mut row = 1;
    for record in &published {
        for line in &record.invoice.items {
            items.write_string(row, 0, &record.id)?;
            items.write_string(row, 1, &record.invoice.invoice_number)?;
            items.write_string(row, 2, line.sku.as_deref().unwrap_or(""))?;
            items.write_string(row, 3, &line.description)?;
            items.write_string(row, 4, line.quantity.as_deref().unwrap_or(""))?;
            if let Some(value) = line.unit_price_cents {
                items.write_number_with_format(row, 5, value as f64 / 100., &cash)?;
            }
            if let Some(value) = line.amount_cents {
                items.write_number_with_format(row, 6, value as f64 / 100., &cash)?;
            }
            row += 1;
        }
    }
    Ok(workbook.save_to_buffer()?)
}

#[cfg(test)]
pub fn cached_workbook(
    store: &Store,
    account: &str,
    selected: Option<&str>,
    origin: &str,
) -> Result<Vec<u8>> {
    filtered_workbook(
        store,
        account,
        selected,
        &crate::catalog::Filters::default(),
        "",
        origin,
    )
}
pub fn filtered_workbook(
    store: &Store,
    account: &str,
    selected: Option<&str>,
    filters: &crate::catalog::Filters,
    search: &str,
    origin: &str,
) -> Result<Vec<u8>> {
    // Build outside the database lock, then cache only if the snapshot is still current.
    let db = store.db.lock().unwrap();
    let version: i64 = db
        .query_row(
            "SELECT version FROM workspace_versions WHERE account=?1",
            [account],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    // Version generated content so a deployment never returns an earlier workbook layout.
    let key = crate::storage::hash(&serde_json::to_vec(&(
        "tick-v1", selected, filters, search,
    ))?);
    let cached: Option<Vec<u8>> = db
        .query_row(
            "SELECT workbook FROM export_cache WHERE account=?1 AND store_id=?2 AND origin=?3",
            rusqlite::params![account, key, origin],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(bytes) = cached {
        return store.unseal(account, &bytes);
    }
    let mut stmt = db.prepare("SELECT record FROM invoices WHERE account=?1 AND status IN ('ready','approved') ORDER BY created_at,id")?;
    let records: Vec<Record> = stmt
        .query_map([account], |r| r.get::<_, Vec<u8>>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .iter()
        .map(|b| store.read_json(account, b))
        .collect::<Result<Vec<_>>>()?;
    let mut records: Vec<_> = records
        .into_iter()
        .filter(|r| {
            crate::catalog::matches(r, selected.unwrap_or(""), &search.to_lowercase(), filters)
        })
        .collect();
    crate::catalog::sort_records(&mut records, filters);
    drop(stmt);
    let mut stmt = db.prepare("SELECT id,name FROM stores WHERE workspace=?1 ORDER BY name")?;
    let stores = stmt
        .query_map([account], |r| {
            Ok(serde_json::json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?}))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    drop(db);
    let bytes = build_workbook(&records, &stores, origin)?;
    let db = store.db.lock().unwrap();
    let current: i64 = db
        .query_row(
            "SELECT version FROM workspace_versions WHERE account=?1",
            [account],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    if current != version {
        return Ok(bytes);
    }
    db.execute(
        "INSERT OR REPLACE INTO export_cache(account,store_id,origin,workbook) VALUES (?1,?2,?3,?4)",
        rusqlite::params![account, key, origin, store.seal(account, &bytes)?],
    )?;
    db.execute("DELETE FROM export_cache WHERE account=?1 AND rowid NOT IN (SELECT rowid FROM export_cache WHERE account=?1 ORDER BY rowid DESC LIMIT 16)",[account])?;
    Ok(bytes)
}
