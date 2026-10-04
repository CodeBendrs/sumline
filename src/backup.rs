//! Consistent snapshots of the live database, and a check that a snapshot is restorable.
//!
//! A snapshot contains the encrypted documents and the readable account/workspace metadata. It
//! does not contain the encryption key: keep that file somewhere else, because a snapshot is
//! useless without it and the two together expose every document.

use crate::storage::unseal_with_key;
use anyhow::{Result, anyhow};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};

pub struct Report {
    pub accounts: i64,
    pub workspaces: i64,
    pub invoices: i64,
    /// `None` when the snapshot holds no documents to test the key against.
    pub document_decrypted: Option<bool>,
}

/// Writes `invoice-tracker-<UTC time>.sqlite` into `destination` using `VACUUM INTO`, which
/// produces a transactionally consistent copy even while the server is running.
pub fn create(data_dir: &Path, destination: &Path) -> Result<PathBuf> {
    let source = data_dir.join("invoice-tracker.sqlite");
    if !source.is_file() {
        return Err(anyhow!("No database found in the data directory"));
    }
    std::fs::create_dir_all(destination)?;
    let target = destination.join(format!(
        "invoice-tracker-{}.sqlite",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    if target.exists() {
        return Err(anyhow!("A snapshot with this timestamp already exists"));
    }
    let db = Connection::open(&source)?;
    db.busy_timeout(std::time::Duration::from_secs(30))?;
    db.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(target)
}

/// Opens the snapshot read-only, runs SQLite's integrity check, counts the main tables and, when
/// the snapshot has any documents, decrypts one with the key to prove they belong together.
pub fn verify(snapshot: &Path, key: &[u8]) -> Result<Report> {
    let db = Connection::open_with_flags(
        snapshot,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let integrity: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if integrity != "ok" {
        return Err(anyhow!("The snapshot failed SQLite's integrity check"));
    }
    let count = |table: &str| -> Result<i64> {
        Ok(db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
    };
    let sample: Option<(String, Vec<u8>)> = db
        .query_row("SELECT account,document FROM invoices LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let document_decrypted =
        sample.map(|(workspace, sealed)| unseal_with_key(key, &workspace, &sealed).is_ok());
    Ok(Report {
        accounts: count("accounts")?,
        workspaces: count("workspaces")?,
        invoices: count("invoices")?,
        document_decrypted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Store;

    fn populated() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let user = store
            .register("owner@example.test", "a long enough password", "Store")
            .unwrap();
        let workspace = store.workspace_for_user(&user).unwrap();
        store
            .insert_document(
                &workspace,
                "a.pdf",
                b"%PDF-snapshot-test",
                "upload",
                None,
                None,
            )
            .unwrap();
        (dir, store)
    }

    #[test]
    fn a_snapshot_taken_while_the_store_is_open_restores_with_the_right_key_only() {
        let (dir, _store) = populated();
        let backups = tempfile::tempdir().unwrap();
        let snapshot = create(dir.path(), backups.path()).unwrap();

        let key = std::fs::read(dir.path().join("encryption.key")).unwrap();
        let report = verify(&snapshot, &key).unwrap();
        assert_eq!(
            (report.accounts, report.workspaces, report.invoices),
            (1, 1, 1)
        );
        assert_eq!(report.document_decrypted, Some(true));

        assert_eq!(
            verify(&snapshot, &[7u8; 32]).unwrap().document_decrypted,
            Some(false)
        );
    }

    #[test]
    fn a_corrupt_snapshot_is_rejected() {
        let (dir, _store) = populated();
        let backups = tempfile::tempdir().unwrap();
        let snapshot = create(dir.path(), backups.path()).unwrap();
        let mut bytes = std::fs::read(&snapshot).unwrap();
        let middle = bytes.len() / 2;
        for byte in &mut bytes[middle..middle + 4096] {
            *byte ^= 0xFF;
        }
        std::fs::write(&snapshot, bytes).unwrap();
        let key = std::fs::read(dir.path().join("encryption.key")).unwrap();
        assert!(verify(&snapshot, &key).is_err());
    }

    #[test]
    fn missing_database_is_reported() {
        let empty = tempfile::tempdir().unwrap();
        assert!(create(empty.path(), empty.path()).is_err());
    }
}
