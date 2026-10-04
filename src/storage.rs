use crate::model::{Invoice, Record};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::{Result, anyhow};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
pub fn hash(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[derive(Clone)]
pub struct Store {
    pub db: Arc<Mutex<Connection>>,
    cipher: Arc<Aes256Gcm>,
}

impl Store {
    pub fn open(directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        let external_key = std::env::var_os("INVOICE_ENCRYPTION_KEY_FILE");
        let key_path = external_key
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| directory.join("encryption.key"));
        if external_key.is_some() && !key_path.is_file() {
            return Err(anyhow!("Configured encryption key is missing"));
        }
        let key = if key_path.exists() {
            std::fs::read(&key_path)?
        } else {
            let mut key = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut key);
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            use std::io::Write;
            options.open(&key_path)?.write_all(&key)?;
            key.to_vec()
        };
        if key.len() != 32 {
            return Err(anyhow!("Invalid encryption key"));
        }
        let db = Connection::open(directory.join("invoice-tracker.sqlite"))?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS accounts (id TEXT PRIMARY KEY, email TEXT UNIQUE NOT NULL, password TEXT NOT NULL, store_name TEXT NOT NULL, created_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS workspaces (id TEXT PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS memberships (workspace TEXT NOT NULL REFERENCES workspaces(id), user TEXT NOT NULL REFERENCES accounts(id), role TEXT NOT NULL CHECK(role IN ('owner','member')), PRIMARY KEY(workspace,user));
            CREATE TABLE IF NOT EXISTS stores (id TEXT PRIMARY KEY, workspace TEXT NOT NULL REFERENCES workspaces(id), name TEXT NOT NULL, identifiers TEXT NOT NULL DEFAULT '[]');
            CREATE TABLE IF NOT EXISTS sessions (hash TEXT PRIMARY KEY, account TEXT NOT NULL REFERENCES accounts(id), workspace TEXT NOT NULL REFERENCES workspaces(id), csrf TEXT NOT NULL, expires INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS invoices (id TEXT PRIMARY KEY, account TEXT NOT NULL REFERENCES workspaces(id), sha TEXT NOT NULL, status TEXT NOT NULL, record BLOB NOT NULL, document BLOB NOT NULL, created_at TEXT NOT NULL, UNIQUE(account,sha));
            CREATE INDEX IF NOT EXISTS invoice_account ON invoices(account);
            CREATE TABLE IF NOT EXISTS exports (account TEXT PRIMARY KEY REFERENCES workspaces(id), workbook BLOB NOT NULL, updated_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS connectors (account TEXT NOT NULL REFERENCES workspaces(id), provider TEXT NOT NULL, email TEXT NOT NULL, tokens BLOB NOT NULL, last_sync TEXT, last_error TEXT, PRIMARY KEY(account,provider));
            CREATE TABLE IF NOT EXISTS oauth_states (hash TEXT PRIMARY KEY, account TEXT NOT NULL REFERENCES workspaces(id), session_hash TEXT NOT NULL, provider TEXT NOT NULL, verifier BLOB NOT NULL, expires INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS processed_messages (account TEXT NOT NULL, provider TEXT NOT NULL, message_id TEXT NOT NULL, PRIMARY KEY(account,provider,message_id));
            CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, account TEXT NOT NULL REFERENCES workspaces(id), time TEXT NOT NULL, kind TEXT NOT NULL, message TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS consents (id INTEGER PRIMARY KEY, user TEXT NOT NULL REFERENCES accounts(id), workspace TEXT NOT NULL REFERENCES workspaces(id), kind TEXT NOT NULL, version TEXT NOT NULL, granted_at TEXT NOT NULL, withdrawn_at TEXT);
            CREATE TABLE IF NOT EXISTS invitations (hash TEXT PRIMARY KEY, workspace TEXT NOT NULL REFERENCES workspaces(id), created_by TEXT NOT NULL REFERENCES accounts(id), email TEXT NOT NULL, expires INTEGER NOT NULL);
        ")?;
        crate::reliability::migrate(&db)?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            cipher: Arc::new(Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("Key error"))?),
        })
    }
    pub fn seal(&self, account: &str, bytes: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let mut result = nonce.to_vec();
        result.extend(
            self.cipher
                .encrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: bytes,
                        aad: account.as_bytes(),
                    },
                )
                .map_err(|_| anyhow!("Encryption failed"))?,
        );
        Ok(result)
    }
    pub fn unseal(&self, account: &str, bytes: &[u8]) -> Result<Vec<u8>> {
        if bytes.len() < 12 {
            return Err(anyhow!("Invalid encrypted record"));
        }
        self.cipher
            .decrypt(
                Nonce::from_slice(&bytes[..12]),
                Payload {
                    msg: &bytes[12..],
                    aad: account.as_bytes(),
                },
            )
            .map_err(|_| anyhow!("Decryption failed"))
    }
    pub fn seal_json<T: Serialize>(&self, account: &str, value: &T) -> Result<Vec<u8>> {
        self.seal(account, &serde_json::to_vec(value)?)
    }
    pub fn read_json<T: DeserializeOwned>(&self, account: &str, value: &[u8]) -> Result<T> {
        Ok(serde_json::from_slice(&self.unseal(account, value)?)?)
    }
    pub fn register(&self, email: &str, password: &str, name: &str) -> Result<String> {
        let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
        let encoded = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|_| anyhow!("Password hashing failed"))?
            .to_string();
        let id = uuid::Uuid::new_v4().to_string();
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO accounts VALUES (?1,?2,?3,?4,?5)",
            params![id, email.to_lowercase().trim(), encoded, name, now()],
        )?;
        let workspace = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO workspaces VALUES (?1,?2)",
            params![workspace, name],
        )?;
        tx.execute(
            "INSERT INTO memberships VALUES (?1,?2,'owner')",
            params![workspace, id],
        )?;
        tx.execute(
            "INSERT INTO stores VALUES (?1,?2,?3,'[]')",
            params![uuid::Uuid::new_v4().to_string(), workspace, name],
        )?;
        tx.commit()?;
        Ok(id)
    }
    pub fn login(&self, email: &str, password: &str) -> Result<Option<String>> {
        let row: Option<(String, String)> = self
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT id,password FROM accounts WHERE email=?1",
                [email.to_lowercase().trim()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, encoded)) = row
            && Argon2::default()
                .verify_password(
                    password.as_bytes(),
                    &PasswordHash::new(&encoded).map_err(|_| anyhow!("Password record error"))?,
                )
                .is_ok()
        {
            return Ok(Some(id));
        }
        Ok(None)
    }
    pub fn add_session(&self, account: &str) -> Result<(String, String)> {
        let session = random_token();
        let csrf = random_token();
        let db = self.db.lock().unwrap();
        db.execute(
            "DELETE FROM sessions WHERE expires<?1",
            [chrono::Utc::now().timestamp()],
        )?;
        let workspace: String = db.query_row(
            "SELECT workspace FROM memberships WHERE user=?1 ORDER BY role DESC LIMIT 1",
            [account],
            |r| r.get(0),
        )?;
        db.execute(
            "INSERT INTO sessions VALUES (?1,?2,?3,?4,?5)",
            params![
                hash(session.as_bytes()),
                account,
                workspace,
                csrf,
                chrono::Utc::now().timestamp() + 43200
            ],
        )?;
        Ok((session, csrf))
    }
    pub fn session(&self, session: &str) -> Result<Option<(String, String, String, String)>> {
        Ok(self.db.lock().unwrap().query_row("SELECT s.account,s.workspace,s.csrf,m.role FROM sessions s JOIN memberships m ON m.user=s.account AND m.workspace=s.workspace WHERE s.hash=?1 AND s.expires>?2",params![hash(session.as_bytes()),chrono::Utc::now().timestamp()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?)
    }
    pub fn account(&self, id: &str) -> Result<serde_json::Value> {
        Ok(self.db.lock().unwrap().query_row("SELECT email,store_name FROM accounts WHERE id=?1",[id],|r|Ok(serde_json::json!({"id":id,"email":r.get::<_,String>(0)?,"store_name":r.get::<_,String>(1)?})))?)
    }
    pub fn insert_document(
        &self,
        account: &str,
        name: &str,
        bytes: &[u8],
        source: &str,
        subject: Option<String>,
        sender: Option<String>,
    ) -> Result<Option<String>> {
        self.insert_document_linked(account, name, bytes, source, subject, sender, None)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn insert_document_linked(
        &self,
        account: &str,
        name: &str,
        bytes: &[u8],
        source: &str,
        subject: Option<String>,
        sender: Option<String>,
        parent: Option<String>,
    ) -> Result<Option<String>> {
        let id = uuid::Uuid::new_v4().to_string();
        let record = Record {
            id: id.clone(),
            revision: 0,
            store_id: None,
            original_document_id: parent,
            file_name: name.into(),
            status: "processing".into(),
            imported_at: now(),
            source: source.into(),
            email_subject: subject,
            email_sender: sender,
            invoice: Invoice::default(),
        };
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM invoices WHERE account=?1 AND sha=?2)",
            params![account, hash(bytes)],
            |r| r.get::<_, bool>(0),
        )? {
            return Ok(None);
        }
        let (tenant, global): (i64, i64) = tx.query_row(
            "SELECT SUM(CASE WHEN account=?1 THEN 1 ELSE 0 END),COUNT(*) FROM invoice_jobs",
            [account],
            |r| Ok((r.get::<_, Option<i64>>(0)?.unwrap_or(0), r.get(1)?)),
        )?;
        if tenant >= 1000 || global >= 10000 {
            return Err(anyhow!("The processing queue is full. Try again shortly."));
        }
        let count = tx.execute(
            "INSERT INTO invoices(id,account,sha,status,record,document,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                id,
                account,
                hash(bytes),
                record.status,
                self.seal_json(account, &record)?,
                self.seal(account, bytes)?,
                record.imported_at
            ],
        )?;
        tx.execute(
            "INSERT INTO invoice_jobs(id,account,available_at) VALUES (?1,?2,?3)",
            params![id, account, chrono::Utc::now().timestamp()],
        )?;
        tx.commit()?;
        Ok(if count > 0 { Some(id) } else { None })
    }
    pub fn records(&self, account: &str) -> Result<Vec<Record>> {
        let db = self.db.lock().unwrap();
        let mut stmt =
            db.prepare("SELECT record,revision FROM invoices WHERE account=?1 ORDER BY created_at DESC,id DESC")?;
        let blobs: Vec<(Vec<u8>, i64)> = stmt
            .query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        blobs
            .iter()
            .map(|(b, revision)| {
                let mut record: Record = self.read_json(account, b)?;
                record.revision = *revision;
                Ok(record)
            })
            .collect()
    }
    pub fn record(&self, account: &str, id: &str) -> Result<Option<Record>> {
        let blob: Option<(Vec<u8>, i64)> = self
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT record,revision FROM invoices WHERE id=?1 AND account=?2",
                params![id, account],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        blob.map(|(b, revision)| {
            let mut record: Record = self.read_json(account, &b)?;
            record.revision = revision;
            Ok(record)
        })
        .transpose()
    }
    pub fn document(&self, account: &str, id: &str) -> Result<Option<Vec<u8>>> {
        let blob: Option<Vec<u8>> = self
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT document FROM invoices WHERE id=?1 AND account=?2",
                params![id, account],
                |r| r.get(0),
            )
            .optional()?;
        blob.map(|b| self.unseal(account, &b)).transpose()
    }
    pub fn update_record(&self, account: &str, record: &Record) -> Result<()> {
        self.commit_record(account, record, None).map(|_| ())
    }
    pub fn event(&self, account: &str, kind: &str, message: &str) -> Result<()> {
        self.db.lock().unwrap().execute(
            "INSERT INTO events(account,time,kind,message) VALUES (?1,?2,?3,?4)",
            params![account, now(), kind, message],
        )?;
        Ok(())
    }
    pub fn workspace_for_user(&self, user: &str) -> Result<String> {
        Ok(self.db.lock().unwrap().query_row(
            "SELECT workspace FROM memberships WHERE user=?1 ORDER BY role DESC LIMIT 1",
            [user],
            |r| r.get(0),
        )?)
    }
    pub fn stores(&self, workspace: &str) -> Result<Vec<serde_json::Value>> {
        let db = self.db.lock().unwrap();
        let mut stmt =
            db.prepare("SELECT id,name,identifiers FROM stores WHERE workspace=?1 ORDER BY name")?;
        Ok(stmt.query_map([workspace],|r|Ok(serde_json::json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"identifiers":serde_json::from_str::<serde_json::Value>(&r.get::<_,String>(2)?).unwrap_or_default()})))?.collect::<std::result::Result<_,_>>()?)
    }
    pub fn store_belongs(&self, workspace: &str, id: &str) -> Result<bool> {
        Ok(self.db.lock().unwrap().query_row(
            "SELECT EXISTS(SELECT 1 FROM stores WHERE workspace=?1 AND id=?2)",
            params![workspace, id],
            |r| r.get(0),
        )?)
    }
    pub fn match_store(&self, workspace: &str, text: &str) -> Result<Option<String>> {
        let normalized = normalize(text);
        let matches: Vec<_> = self
            .stores(workspace)?
            .into_iter()
            .filter(|store| {
                store["identifiers"].as_array().is_some_and(|keys| {
                    keys.iter()
                        .filter_map(|v| v.as_str())
                        .any(|key| key.len() >= 8 && normalized.contains(&normalize(key)))
                })
            })
            .collect();
        Ok(if matches.len() == 1 {
            matches[0]["id"].as_str().map(str::to_string)
        } else {
            None
        })
    }
    pub fn consent(&self, user: &str, workspace: &str, kind: &str) -> Result<()> {
        self.db.lock().unwrap().execute(
            "INSERT INTO consents(user,workspace,kind,version,granted_at) VALUES (?1,?2,?3,?4,?5)",
            params![user, workspace, kind, crate::privacy::POLICY_VERSION, now()],
        )?;
        Ok(())
    }
}

fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_data_and_invoice_ids_are_bound_to_account() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let a_user = store
            .register("a@example.com", "correct horse battery staple", "A")
            .unwrap();
        let b_user = store
            .register("b@example.com", "correct horse battery staple", "B")
            .unwrap();
        let a = store.workspace_for_user(&a_user).unwrap();
        let b = store.workspace_for_user(&b_user).unwrap();
        let id = store
            .insert_document(&a, "a.pdf", b"%PDF-test", "upload", None, None)
            .unwrap()
            .unwrap();
        assert!(store.record(&b, &id).unwrap().is_none());
        assert!(store.document(&b, &id).unwrap().is_none());
        assert!(
            store
                .insert_document(&a, "copy.pdf", b"%PDF-test", "upload", None, None)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .insert_document(&b, "copy.pdf", b"%PDF-test", "upload", None, None)
                .unwrap()
                .is_some()
        );
        let sealed = store.seal(&a, b"private invoice").unwrap();
        assert!(store.unseal(&b, &sealed).is_err());
        assert_eq!(store.unseal(&a, &sealed).unwrap(), b"private invoice");
        assert!(store.login("a@example.com", "wrong").unwrap().is_none());
    }
    #[test]
    fn routing_requires_exactly_one_store_in_the_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let user = store
            .register(
                "router@example.com",
                "correct horse battery staple",
                "Retail",
            )
            .unwrap();
        let workspace = store.workspace_for_user(&user).unwrap();
        let first = store.stores(&workspace).unwrap()[0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        store
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE stores SET identifiers='[\"2500 Douglas\"]' WHERE id=?1",
                [&first],
            )
            .unwrap();
        let second = uuid::Uuid::new_v4().to_string();
        store
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO stores VALUES (?1,?2,'Downtown','[\"999 Main Street\"]')",
                params![second, workspace],
            )
            .unwrap();
        assert_eq!(
            store
                .match_store(&workspace, "Ship to 2500 DOUGLAS Street")
                .unwrap(),
            Some(first)
        );
        assert_eq!(
            store
                .match_store(&workspace, "Ship to 999 Main Street")
                .unwrap(),
            Some(second)
        );
        assert_eq!(
            store
                .match_store(&workspace, "2500 Douglas / 999 Main Street")
                .unwrap(),
            None
        );
        assert_eq!(
            store.match_store(&workspace, "Unknown address").unwrap(),
            None
        );
    }
}
