use crate::{
    App,
    extraction::{Context, parse_invoice, parse_invoice_with},
};
use anyhow::{Result, anyhow};
use std::{io::Write, path::Path, process::Stdio};

pub fn extract_child(path: &Path) -> Result<serde_json::Value> {
    #[cfg(unix)]
    unsafe {
        let cpu = libc::rlimit {
            rlim_cur: 20,
            rlim_max: 20,
        };
        libc::setrlimit(libc::RLIMIT_CPU, &cpu);
        let files = libc::rlimit {
            rlim_cur: 64,
            rlim_max: 64,
        };
        libc::setrlimit(libc::RLIMIT_NOFILE, &files);
        let memory = libc::rlimit {
            rlim_cur: 1024 * 1024 * 1024,
            rlim_max: 1024 * 1024 * 1024,
        };
        #[cfg(not(target_os = "macos"))]
        libc::setrlimit(libc::RLIMIT_AS, &memory);
        #[cfg(target_os = "macos")]
        let _ = memory;
    }
    let bytes = std::fs::read(path)?;
    if bytes.len() > 20 * 1024 * 1024 || !bytes.starts_with(b"%PDF-") {
        return Err(anyhow!("Not a supported PDF"));
    }
    // Poppler handles malformed cross-reference tables seen in real vendor PDFs.
    // The Rust extractor is retained as a fallback for hosts without Poppler.
    let poppler = std::process::Command::new("pdftotext")
        .args(["-layout", "-enc", "UTF-8"])
        .arg(path)
        .arg("-")
        .output();
    let (text, pages) = match poppler {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8(output.stdout)?;
            let info = std::process::Command::new("pdfinfo").arg(path).output()?;
            if !info.status.success() {
                return Err(anyhow!("Invalid PDF metadata"));
            }
            let info = String::from_utf8_lossy(&info.stdout);
            if info
                .lines()
                .any(|line| line.starts_with("Encrypted:") && !line.contains("no"))
            {
                return Err(anyhow!("Password-protected PDF"));
            }
            let pages = info
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Pages:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            (text, pages)
        }
        _ => {
            let document = lopdf::Document::load_mem(&bytes)?;
            if document.is_encrypted() {
                return Err(anyhow!("Password-protected PDF"));
            }
            (
                pdf_extract::extract_text_from_mem(&bytes)?,
                document.get_pages().len(),
            )
        }
    };
    if pages == 0 || pages > 50 {
        return Err(anyhow!("PDF must have 1 to 50 pages"));
    }
    if text.len() > 1024 * 1024 {
        return Err(anyhow!("PDF text exceeds the extraction limit"));
    }
    let page_texts: Vec<_> = text
        .split('\u{c}')
        .filter(|v| !v.trim().is_empty())
        .collect();
    let mut groups: Vec<(usize, usize, String)> = Vec::new();
    for (index, page) in page_texts.iter().enumerate() {
        let parsed = parse_invoice(page, 1);
        let number = parsed.invoice_number;
        if let Some(last) = groups.last_mut()
            && (number.is_empty() || last.2 == number)
        {
            last.1 = index + 1;
            continue;
        }
        groups.push((index + 1, index + 1, number));
    }
    let mut segments = Vec::new();
    if groups.len() > 1 && groups.iter().all(|g| !g.2.is_empty()) && page_texts.len() == pages {
        for (first, last, _) in groups {
            let pdf = split_pdf(path, first, last)?;
            use base64::Engine;
            segments.push(serde_json::json!({"text":page_texts[first-1..last].join("\n"),"pages":last-first+1,"pdf":base64::engine::general_purpose::STANDARD.encode(pdf)}));
        }
    }
    Ok(serde_json::json!({"text":text,"pages":pages,"segments":segments}))
}

fn split_pdf(path: &Path, first: usize, last: usize) -> Result<Vec<u8>> {
    let directory = tempfile::tempdir()?;
    let template = directory.path().join("page-%d.pdf");
    let status = std::process::Command::new("pdfseparate")
        .arg("-f")
        .arg(first.to_string())
        .arg("-l")
        .arg(last.to_string())
        .arg(path)
        .arg(&template)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(anyhow!("PDF bundle could not be separated"));
    }
    if first == last {
        return Ok(std::fs::read(
            directory.path().join(format!("page-{first}.pdf")),
        )?);
    }
    let mut command = std::process::Command::new("pdfunite");
    for page in first..=last {
        command.arg(directory.path().join(format!("page-{page}.pdf")));
    }
    let output = directory.path().join("invoice.pdf");
    if !command
        .arg(&output)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success()
    {
        return Err(anyhow!("PDF pages could not be joined"));
    }
    Ok(std::fs::read(output)?)
}

/// The workspace's own store names and addresses. An invoice is addressed to one of them, so
/// text containing them is the customer and not the supplier.
fn customer_context(app: &App, workspace: &str) -> Result<Context> {
    let collapse = |v: &str| {
        v.to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut customer = Vec::new();
    for store in app.store.stores(workspace)? {
        if let Some(name) = store["name"].as_str() {
            customer.push(collapse(name));
        }
        for identifier in store["identifiers"].as_array().into_iter().flatten() {
            if let Some(text) = identifier.as_str() {
                customer.push(collapse(text));
            }
        }
    }
    Ok(Context {
        customer,
        known_suppliers: app.store.known_suppliers(workspace)?,
        ..Default::default()
    })
}

pub async fn process(app: &App, job: &crate::reliability::Job) -> Result<()> {
    let account = job.account.as_str();
    let id = job.id.as_str();
    let mut record = app
        .store
        .record(account, id)?
        .ok_or_else(|| anyhow!("Record unavailable"))?;
    let bytes = app
        .store
        .document(account, id)?
        .ok_or_else(|| anyhow!("Document unavailable"))?;
    let mut temporary = tempfile::NamedTempFile::new()?;
    temporary.write_all(&bytes)?;
    temporary.flush()?;
    let child = tokio::process::Command::new(std::env::current_exe()?)
        .arg("--extract")
        .arg(temporary.path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/opt/homebrew/bin")
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(25), child.wait_with_output()).await;
    match output {
        Ok(Ok(output)) if output.status.success() => {
            let result: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            if let Some(segments) = result["segments"].as_array().filter(|v| v.len() > 1) {
                use base64::Engine;
                for (index, segment) in segments.iter().enumerate() {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(segment["pdf"].as_str().unwrap_or(""))?;
                    let name = format!(
                        "{} - invoice {}.pdf",
                        record.file_name.trim_end_matches(".pdf"),
                        index + 1
                    );
                    app.store.insert_document_linked(
                        account,
                        &name,
                        &bytes,
                        &record.source,
                        record.email_subject.clone(),
                        record.email_sender.clone(),
                        Some(id.into()),
                    )?;
                    app.queue.notify_waiters();
                }
                record.status = "bundle".into();
                record.invoice.warnings = vec![format!(
                    "This attachment contains {} separate invoices. Each has its own archive entry and store assignment.",
                    segments.len()
                )];
                app.store.finish_job(job, &record)?;
                return Ok(());
            }
            let text = result["text"].as_str().unwrap_or("");
            record.invoice = parse_invoice_with(
                text,
                result["pages"].as_u64().unwrap_or(1) as usize,
                &customer_context(app, account)?,
            );
            let looks_like_invoice = regex::Regex::new(r"(?i)invoice|facture|credit\s+note")
                .unwrap()
                .is_match(text);
            if !looks_like_invoice && text.trim().len() >= 30 {
                record.status = "excluded".into();
                record.invoice.warnings=vec!["This PDF does not appear to be an invoice. It was excluded from the spreadsheet.".into()];
            } else {
                if record.store_id.is_none() {
                    record.store_id = app.store.match_store(account, text)?;
                }
                if record.store_id.is_none() {
                    record.invoice.warnings.push("Choose the store. Delivery details did not match exactly one configured store.".into());
                }
                record.status = if record.invoice.warnings.is_empty() {
                    "ready"
                } else {
                    "review"
                }
                .into();
            }
        }
        _ => {
            record.status = "review".into();
            record.invoice.warnings=vec!["This PDF could not be read within the processing limits. Check that it is readable, unencrypted, and at most 50 pages.".into()];
        }
    }
    // Only the worker may complete a processing record; review actions are blocked while processing.
    record = app.store.finish_job(job, &record)?;
    app.store.event(
        account,
        &record.status,
        match record.status.as_str() {
            "ready" => "Invoice added to the spreadsheet.",
            "excluded" => "A non-invoice PDF was excluded.",
            _ => "An invoice needs review.",
        },
    )?;
    Ok(())
}

pub async fn run(app: App) {
    loop {
        match app.store.claim_job() {
            Ok(Some(job)) => {
                if process(&app, &job).await.is_err() {
                    let _ = app.store.fail_job(&job);
                }
            }
            _ => {
                tokio::select! { _ = app.queue.notified() => {}, _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {} }
            }
        }
    }
}
