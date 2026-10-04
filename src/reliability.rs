//! Durable jobs and transactional invoice changes. SQLite is a single-host deployment.
use crate::{
    model::Record,
    storage::{Store, hash, now},
};
use anyhow::{Result, anyhow};
use axum::http::HeaderMap;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

#[derive(Debug)]
pub struct Conflict(pub String);
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Conflict {}

pub fn migrate(db: &Connection) -> Result<()> {
    let columns: Vec<String> = db
        .prepare("PRAGMA table_info(invoices)")?
        .query_map([], |r| r.get(1))?
        .collect::<std::result::Result<_, _>>()?;
    if !columns.iter().any(|v| v == "revision") {
        db.execute(
            "ALTER TABLE invoices ADD COLUMN revision INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !columns.iter().any(|v| v == "store_id") {
        db.execute("ALTER TABLE invoices ADD COLUMN store_id TEXT", [])?;
    }
    db.execute_batch("CREATE TABLE IF NOT EXISTS invoice_jobs(id TEXT PRIMARY KEY REFERENCES invoices(id) ON DELETE CASCADE,account TEXT NOT NULL,available_at INTEGER NOT NULL,attempts INTEGER NOT NULL DEFAULT 0,lease_token TEXT,lease_until INTEGER);
        CREATE INDEX IF NOT EXISTS invoice_jobs_due ON invoice_jobs(available_at,lease_until);
        CREATE INDEX IF NOT EXISTS invoice_jobs_workspace ON invoice_jobs(account);
        CREATE INDEX IF NOT EXISTS invoice_list ON invoices(account,status,store_id,created_at);
        CREATE TABLE IF NOT EXISTS invoice_intents(account TEXT NOT NULL,intent TEXT NOT NULL,payload TEXT NOT NULL,response BLOB NOT NULL,expires INTEGER NOT NULL,PRIMARY KEY(account,intent));
        CREATE TABLE IF NOT EXISTS provider_cooldowns(token TEXT PRIMARY KEY,until INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS workspace_versions(account TEXT PRIMARY KEY,version INTEGER NOT NULL);
        INSERT OR IGNORE INTO workspace_versions SELECT id,0 FROM workspaces;
        CREATE TRIGGER IF NOT EXISTS invoice_version_insert AFTER INSERT ON invoices BEGIN INSERT INTO workspace_versions VALUES(NEW.account,1) ON CONFLICT(account) DO UPDATE SET version=version+1; END;
        CREATE TRIGGER IF NOT EXISTS invoice_version_update AFTER UPDATE ON invoices BEGIN INSERT INTO workspace_versions VALUES(NEW.account,1) ON CONFLICT(account) DO UPDATE SET version=version+1; END;
        CREATE TRIGGER IF NOT EXISTS invoice_version_delete AFTER DELETE ON invoices BEGIN INSERT INTO workspace_versions VALUES(OLD.account,1) ON CONFLICT(account) DO UPDATE SET version=version+1; END;
        CREATE TRIGGER IF NOT EXISTS store_version_update AFTER UPDATE ON stores BEGIN INSERT INTO workspace_versions VALUES(NEW.workspace,1) ON CONFLICT(account) DO UPDATE SET version=version+1; END;
        CREATE TRIGGER IF NOT EXISTS store_version_insert AFTER INSERT ON stores BEGIN INSERT INTO workspace_versions VALUES(NEW.workspace,1) ON CONFLICT(account) DO UPDATE SET version=version+1; END;
        CREATE TABLE IF NOT EXISTS query_cache(account TEXT NOT NULL,key TEXT NOT NULL,response BLOB NOT NULL,created_at INTEGER NOT NULL,PRIMARY KEY(account,key));
        CREATE TRIGGER IF NOT EXISTS query_cache_insert AFTER INSERT ON invoices BEGIN DELETE FROM query_cache WHERE account=NEW.account; END;
        CREATE TRIGGER IF NOT EXISTS query_cache_update AFTER UPDATE ON invoices BEGIN DELETE FROM query_cache WHERE account=NEW.account; END;
        CREATE TRIGGER IF NOT EXISTS query_cache_delete AFTER DELETE ON invoices BEGIN DELETE FROM query_cache WHERE account=OLD.account; END;
        CREATE TRIGGER IF NOT EXISTS query_store_update AFTER UPDATE ON stores BEGIN DELETE FROM query_cache WHERE account=NEW.workspace; END;
        CREATE TABLE IF NOT EXISTS export_cache(account TEXT NOT NULL,store_id TEXT NOT NULL,origin TEXT NOT NULL,workbook BLOB NOT NULL,PRIMARY KEY(account,store_id,origin));
        CREATE TRIGGER IF NOT EXISTS invoice_cache_insert AFTER INSERT ON invoices BEGIN DELETE FROM export_cache WHERE account=NEW.account; END;
        CREATE TRIGGER IF NOT EXISTS invoice_cache_update AFTER UPDATE ON invoices BEGIN DELETE FROM export_cache WHERE account=NEW.account; END;
        CREATE TRIGGER IF NOT EXISTS invoice_cache_delete AFTER DELETE ON invoices BEGIN DELETE FROM export_cache WHERE account=OLD.account; END;
        CREATE TRIGGER IF NOT EXISTS store_cache_update AFTER UPDATE ON stores BEGIN DELETE FROM export_cache WHERE account=NEW.workspace; END;
        CREATE TRIGGER IF NOT EXISTS store_cache_insert AFTER INSERT ON stores BEGIN DELETE FROM export_cache WHERE account=NEW.workspace; END;
        DELETE FROM exports;
        INSERT OR IGNORE INTO invoice_jobs(id,account,available_at) SELECT id,account,0 FROM invoices WHERE status='processing';")?;
    Ok(())
}

pub struct Intent {
    key: String,
    payload: String,
}
impl Intent {
    pub fn from_headers(
        headers: &HeaderMap,
        user: &str,
        id: &str,
        body: &[u8],
    ) -> Result<Option<Self>> {
        let Some(value) = headers.get("idempotency-key") else {
            return Ok(None);
        };
        let key = value.to_str()?;
        if !(16..=128).contains(&key.len())
            || !key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(anyhow!("Invalid key"));
        }
        Ok(Some(Self {
            key: hash(format!("{user}:{id}:approve:{key}").as_bytes()),
            payload: hash(body),
        }))
    }
}
#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub account: String,
    pub token: String,
    pub attempts: i64,
}

impl Store {
    pub fn backfill_store_metadata(&self) -> Result<()> {
        let db = self.db.lock().unwrap();
        let mut stmt=db.prepare("SELECT id,account,record FROM invoices WHERE store_id IS NULL AND status!='processing'")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (id, account, blob) in rows {
            let record: Record = self.read_json(&account, &blob)?;
            if let Some(store) = record.store_id {
                db.execute(
                    "UPDATE invoices SET store_id=?1 WHERE id=?2",
                    params![store, id],
                )?;
            }
        }
        Ok(())
    }
    fn replay_tx(&self, tx: &Connection, account: &str, intent: &Intent) -> Result<Option<Record>> {
        let saved:Option<(String,Vec<u8>)>=tx.query_row("SELECT payload,response FROM invoice_intents WHERE account=?1 AND intent=?2 AND expires>?3",params![account,intent.key,chrono::Utc::now().timestamp()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((payload, bytes)) = saved {
            if payload != intent.payload {
                return Err(Conflict(
                    "This retry key was already used for different changes. Start a new save."
                        .into(),
                )
                .into());
            }
            return Ok(Some(self.read_json(account, &bytes)?));
        }
        Ok(None)
    }
    pub fn replay(&self, account: &str, intent: &Intent) -> Result<Option<Record>> {
        self.replay_tx(&self.db.lock().unwrap(), account, intent)
    }
    fn write_tx(&self, tx: &Transaction<'_>, account: &str, record: &Record) -> Result<Record> {
        let mut next = record.clone();
        next.revision += 1;
        if ["ready", "approved"].contains(&next.status.as_str()) {
            let mut stmt=tx.prepare("SELECT record FROM invoices WHERE account=?1 AND id!=?2 AND store_id=?3 AND status IN ('ready','approved')")?;
            let blobs = stmt
                .query_map(params![account, record.id, record.store_id], |r| {
                    r.get::<_, Vec<u8>>(0)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for blob in blobs {
                let other: Record = self.read_json(account, &blob)?;
                if other
                    .invoice
                    .supplier
                    .trim()
                    .eq_ignore_ascii_case(next.invoice.supplier.trim())
                    && other
                        .invoice
                        .invoice_number
                        .trim()
                        .eq_ignore_ascii_case(next.invoice.invoice_number.trim())
                {
                    return Err(Conflict("This store already has this supplier's invoice number in the spreadsheet. Review the duplicate.".into()).into());
                }
            }
        }
        let changed=tx.execute("UPDATE invoices SET status=?1,record=?2,revision=revision+1,store_id=?3 WHERE id=?4 AND account=?5 AND revision=?6",params![next.status,self.seal_json(account,&next)?,next.store_id,next.id,account,record.revision])?;
        if changed != 1 {
            return Err(Conflict("This invoice changed. Reopen it before saving.".into()).into());
        }
        Ok(next)
    }
    pub fn commit_record(
        &self,
        account: &str,
        record: &Record,
        intent: Option<&Intent>,
    ) -> Result<Record> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(intent) = intent
            && let Some(saved) = self.replay_tx(&tx, account, intent)?
        {
            return Ok(saved);
        }
        let next = self.write_tx(&tx, account, record)?;
        if let Some(intent) = intent {
            tx.execute(
                "DELETE FROM invoice_intents WHERE expires<=?1",
                [chrono::Utc::now().timestamp()],
            )?;
            tx.execute(
                "INSERT INTO invoice_intents VALUES (?1,?2,?3,?4,?5)",
                params![
                    account,
                    intent.key,
                    intent.payload,
                    self.seal_json(account, &next)?,
                    chrono::Utc::now().timestamp() + 86400
                ],
            )?;
        }
        tx.execute("INSERT INTO events(account,time,kind,message) VALUES (?1,?2,?3,'Invoice record updated.')",params![account,now(),next.status])?;
        tx.commit()?;
        Ok(next)
    }
    pub fn claim_job(&self) -> Result<Option<Job>> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let time = chrono::Utc::now().timestamp();
        let row:Option<(String,String,i64)>=tx.query_row("SELECT id,account,attempts FROM invoice_jobs WHERE available_at<=?1 AND (lease_token IS NULL OR lease_until<=?1) ORDER BY available_at,id LIMIT 1",[time],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((id, account, attempts)) = row else {
            return Ok(None);
        };
        let token = crate::storage::random_token();
        tx.execute(
            "UPDATE invoice_jobs SET attempts=attempts+1,lease_token=?1,lease_until=?2 WHERE id=?3",
            params![token, time + 120, id],
        )?;
        tx.commit()?;
        Ok(Some(Job {
            id,
            account,
            token,
            attempts: attempts + 1,
        }))
    }
    fn owns_job(tx: &Transaction<'_>, job: &Job) -> Result<bool> {
        Ok(tx.query_row("SELECT EXISTS(SELECT 1 FROM invoice_jobs WHERE id=?1 AND account=?2 AND lease_token=?3 AND lease_until>?4)",params![job.id,job.account,job.token,chrono::Utc::now().timestamp()],|r|r.get(0))?)
    }
    pub fn finish_job(&self, job: &Job, record: &Record) -> Result<Record> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !Self::owns_job(&tx, job)? {
            return Err(Conflict("Job lease expired".into()).into());
        }
        let result = self.write_tx(&tx, &job.account, record);
        let saved = match result {
            Ok(next) => next,
            Err(e) if e.downcast_ref::<Conflict>().is_some() => {
                let mut review = record.clone();
                review.status = "review".into();
                review.invoice.warnings.push("This supplier's invoice number is already in this store's spreadsheet. Review both documents.".into());
                self.write_tx(&tx, &job.account, &review)?
            }
            Err(e) => return Err(e),
        };
        tx.execute(
            "DELETE FROM invoice_jobs WHERE id=?1 AND lease_token=?2",
            params![job.id, job.token],
        )?;
        tx.commit()?;
        Ok(saved)
    }
    pub fn fail_job(&self, job: &Job) -> Result<()> {
        let Some(mut record) = self.record(&job.account, &job.id)? else {
            return Ok(());
        };
        if job.attempts >= 3 {
            record.status = "review".into();
            record.invoice.warnings=vec!["Processing could not finish. Download the PDF to check it, or enter its details here.".into()];
            return self.finish_job(job, &record).map(|_| ());
        }
        let db = self.db.lock().unwrap();
        db.execute("UPDATE invoice_jobs SET lease_token=NULL,lease_until=NULL,available_at=?1 WHERE id=?2 AND lease_token=?3",params![chrono::Utc::now().timestamp()+5_i64.pow(job.attempts as u32),job.id,job.token])?;
        Ok(())
    }
}

impl Store {
    #[cfg(test)]
    pub fn invoice_page(
        &self,
        account: &str,
        view: &str,
        selected: &str,
        search: &str,
        page: usize,
    ) -> Result<serde_json::Value> {
        self.filtered_page(
            account,
            view,
            selected,
            search,
            page,
            &crate::catalog::Filters::default(),
        )
    }
    pub fn delete_record(&self, account: &str, record: &Record) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if tx.execute(
            "DELETE FROM invoices WHERE account=?1 AND id=?2 AND revision=?3",
            params![account, record.id, record.revision],
        )? != 1
        {
            return Err(Conflict("This invoice changed. Reopen it before deleting.".into()).into());
        }
        tx.execute("DELETE FROM invoice_intents WHERE account=?1", [account])?;
        let mut stmt = tx.prepare("SELECT record,revision FROM invoices WHERE account=?1")?;
        let rows = stmt
            .query_map([account], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);
        for (blob, revision) in rows {
            let mut child: Record = self.read_json(account, &blob)?;
            if child.original_document_id.as_deref() == Some(&record.id) {
                child.original_document_id = None;
                child.revision = revision;
                self.write_tx(&tx, account, &child)?;
            }
        }
        tx.execute("INSERT INTO events(account,time,kind,message) VALUES (?1,?2,'deleted','Invoice deleted from the active archive.')",params![account,now()])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store.db.lock().unwrap().execute_batch("INSERT INTO workspaces VALUES ('a','A'),('b','B');INSERT INTO stores VALUES ('s1','a','First','[]'),('s2','a','Second','[]'),('s3','b','Other','[]');").unwrap();
        (directory, store)
    }
    fn add(store: &Store, account: &str, bytes: &[u8]) -> Record {
        let id = store
            .insert_document(account, "invoice.pdf", bytes, "Upload", None, None)
            .unwrap()
            .unwrap();
        store.record(account, &id).unwrap().unwrap()
    }
    #[test]
    fn jobs_survive_reopen_and_expired_workers_cannot_commit() {
        let (directory, store) = fixture();
        let record = add(&store, "a", b"%PDF-first");
        let old = store.claim_job().unwrap().unwrap();
        assert!(store.claim_job().unwrap().is_none());
        store
            .db
            .lock()
            .unwrap()
            .execute("UPDATE invoice_jobs SET lease_until=0", [])
            .unwrap();
        let reopened = Store::open(directory.path()).unwrap();
        let new = reopened.claim_job().unwrap().unwrap();
        assert_ne!(old.token, new.token);
        assert_eq!(new.attempts, 2);
        let mut done = record.clone();
        done.status = "review".into();
        assert!(store.finish_job(&old, &done).is_err());
        assert_eq!(
            store.record("a", &record.id).unwrap().unwrap().status,
            "processing"
        );
        reopened.finish_job(&new, &done).unwrap();
        assert!(store.claim_job().unwrap().is_none());
        assert_eq!(store.record("a", &record.id).unwrap().unwrap().revision, 1);
    }
    #[test]
    fn independent_connections_claim_each_job_once() {
        let (directory, store) = fixture();
        add(&store, "a", b"%PDF-first");
        let other = Store::open(directory.path()).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let one = barrier.clone();
        let thread = std::thread::spawn(move || {
            one.wait();
            other.claim_job().unwrap()
        });
        barrier.wait();
        let first = store.claim_job().unwrap();
        let second = thread.join().unwrap();
        assert_eq!(
            usize::from(first.is_some()) + usize::from(second.is_some()),
            1
        );
    }
    #[test]
    fn review_retry_is_atomic_payload_bound_and_workspace_scoped() {
        let (_directory, store) = fixture();
        let mut record = add(&store, "a", b"%PDF-first");
        record.status = "review".into();
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", "abcdefghijklmnop".parse().unwrap());
        let intent = Intent::from_headers(&headers, "user", &record.id, b"first")
            .unwrap()
            .unwrap();
        let saved = store.commit_record("a", &record, Some(&intent)).unwrap();
        let retried = store.commit_record("a", &record, Some(&intent)).unwrap();
        assert_eq!(saved.revision, retried.revision);
        assert_eq!(store.record("a", &record.id).unwrap().unwrap().revision, 1);
        assert!(store.replay("b", &intent).unwrap().is_none());
        let changed = Intent::from_headers(&headers, "user", &record.id, b"changed")
            .unwrap()
            .unwrap();
        assert!(store.replay("a", &changed).is_err());
        assert!(store.commit_record("a", &record, None).is_err());
    }
    #[test]
    fn publication_duplicates_are_per_store_and_atomic() {
        let (directory, store) = fixture();
        let mut first = add(&store, "a", b"%PDF-first");
        let mut second = add(&store, "a", b"%PDF-second");
        for record in [&mut first, &mut second] {
            record.status = "ready".into();
            record.store_id = Some("s1".into());
            record.invoice.supplier = "Supplier".into();
            record.invoice.invoice_number = "123".into();
        }
        let other = Store::open(directory.path()).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let one = barrier.clone();
        let second_copy = second.clone();
        let thread = std::thread::spawn(move || {
            one.wait();
            other.commit_record("a", &second_copy, None).is_ok()
        });
        barrier.wait();
        let a = store.commit_record("a", &first, None).is_ok();
        let b = thread.join().unwrap();
        assert_ne!(a, b);
        let mut loser = if a { second } else { first };
        loser.store_id = Some("s2".into());
        assert!(store.commit_record("a", &loser, None).is_ok());
    }
    #[test]
    fn bundle_links_exist_before_the_child_can_be_claimed() {
        let (_directory, store) = fixture();
        let parent = add(&store, "a", b"%PDF-parent");
        let id = store
            .insert_document_linked(
                "a",
                "part.pdf",
                b"%PDF-child",
                "Upload",
                None,
                None,
                Some(parent.id.clone()),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            store
                .record("a", &id)
                .unwrap()
                .unwrap()
                .original_document_id,
            Some(parent.id)
        );
    }
    #[test]
    fn pages_are_bounded_and_workspace_isolated_and_cache_invalidates() {
        let (_directory, store) = fixture();
        for index in 0..51 {
            add(&store, "a", format!("%PDF-{index}").as_bytes());
        }
        add(&store, "b", b"%PDF-other");
        let first = store.invoice_page("a", "invoices", "", "", 1).unwrap();
        let second = store.invoice_page("a", "invoices", "", "", 2).unwrap();
        assert_eq!(first["invoices"].as_array().unwrap().len(), 50);
        assert_eq!(second["invoices"].as_array().unwrap().len(), 1);
        assert_eq!(first["total"], 51);
        let bytes = crate::spreadsheet::cached_workbook(&store, "a", None, "http://localhost:3030")
            .unwrap();
        assert!(!bytes.is_empty());
        assert_eq!(
            store
                .db
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM export_cache", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let mut record = store.records("a").unwrap().remove(0);
        record.status = "review".into();
        store.commit_record("a", &record, None).unwrap();
        assert_eq!(
            store
                .db
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM export_cache", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let matches = store.invoice_page("a", "invoices", "", "other", 1).unwrap();
        assert_eq!(matches["total"], 0);
    }
}
