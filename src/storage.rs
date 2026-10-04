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

/// Email verification state and single-use verification/reset tokens.
fn migrate_accounts(db: &Connection) -> Result<()> {
    let columns: Vec<String> = db
        .prepare("PRAGMA table_info(accounts)")?
        .query_map([], |r| r.get(1))?
        .collect::<std::result::Result<_, _>>()?;
    if !columns.iter().any(|v| v == "email_verified_at") {
        db.execute("ALTER TABLE accounts ADD COLUMN email_verified_at TEXT", [])?;
    }
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS email_tokens(hash TEXT PRIMARY KEY, account TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE, purpose TEXT NOT NULL CHECK(purpose IN ('verify','reset')), email TEXT NOT NULL, created INTEGER NOT NULL, expires INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS email_tokens_account ON email_tokens(account,purpose);
        CREATE TABLE IF NOT EXISTS supplier_profiles(workspace TEXT NOT NULL REFERENCES workspaces(id), key TEXT NOT NULL, supplier TEXT NOT NULL, approved_at TEXT NOT NULL, PRIMARY KEY(workspace,key));",
    )?;
    Ok(())
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| anyhow!("Password hashing failed"))?
        .to_string())
}

fn unseal_with_cipher(cipher: &Aes256Gcm, account: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() < 12 {
        return Err(anyhow!("Invalid encrypted record"));
    }
    cipher
        .decrypt(
            Nonce::from_slice(&bytes[..12]),
            Payload {
                msg: &bytes[12..],
                aad: account.as_bytes(),
            },
        )
        .map_err(|_| anyhow!("Decryption failed"))
}

/// Decrypts a stored value with a key file's contents, without opening a `Store`. Used to prove
/// that a backup and a key belong together.
pub fn unseal_with_key(key: &[u8], account: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("Invalid encryption key"))?;
    unseal_with_cipher(&cipher, account, bytes)
}

/// Minimum seconds between two emails of the same kind for one account.
pub const EMAIL_TOKEN_COOLDOWN_SECONDS: i64 = 60;

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
        migrate_accounts(&db)?;
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
        unseal_with_cipher(&self.cipher, account, bytes)
    }
    pub fn seal_json<T: Serialize>(&self, account: &str, value: &T) -> Result<Vec<u8>> {
        self.seal(account, &serde_json::to_vec(value)?)
    }
    pub fn read_json<T: DeserializeOwned>(&self, account: &str, value: &[u8]) -> Result<T> {
        Ok(serde_json::from_slice(&self.unseal(account, value)?)?)
    }
    pub fn register(&self, email: &str, password: &str, name: &str) -> Result<String> {
        let encoded = hash_password(password)?;
        let id = uuid::Uuid::new_v4().to_string();
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO accounts(id,email,password,store_name,created_at) VALUES (?1,?2,?3,?4,?5)",
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
        Ok(self.db.lock().unwrap().query_row("SELECT email,store_name,email_verified_at IS NOT NULL FROM accounts WHERE id=?1",[id],|r|Ok(serde_json::json!({"id":id,"email":r.get::<_,String>(0)?,"store_name":r.get::<_,String>(1)?,"email_verified":r.get::<_,bool>(2)?})))?)
    }
    /// Remembers the supplier a person confirmed, under each identifier found in the document
    /// and under the confirmed name itself. The latest approval for an identifier wins.
    pub fn remember_supplier(
        &self,
        workspace: &str,
        keys: &[String],
        supplier: &str,
    ) -> Result<()> {
        let supplier = supplier.trim();
        if supplier.is_empty() {
            return Ok(());
        }
        let mut all: std::collections::BTreeSet<String> =
            keys.iter().filter(|k| k.len() <= 200).cloned().collect();
        all.insert(crate::supplier::name_key(supplier));
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        for key in all {
            tx.execute(
                "INSERT INTO supplier_profiles(workspace,key,supplier,approved_at) VALUES (?1,?2,?3,?4) ON CONFLICT(workspace,key) DO UPDATE SET supplier=excluded.supplier,approved_at=excluded.approved_at",
                params![workspace, key, supplier, now()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// One-time catch-up for approvals made before supplier memory existed: if nothing has been
    /// remembered yet, remember the name on every approved invoice. Those names were confirmed
    /// by a person, so recognising them is safe; it does nothing once any profile exists.
    pub fn backfill_supplier_profiles(&self) -> Result<usize> {
        let already: i64 = self.db.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM supplier_profiles",
            [],
            |r| r.get(0),
        )?;
        if already > 0 {
            return Ok(0);
        }
        let workspaces: Vec<String> = {
            let db = self.db.lock().unwrap();
            let mut statement =
                db.prepare("SELECT DISTINCT account FROM invoices WHERE status='approved'")?;
            statement
                .query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        let mut remembered = 0;
        for workspace in workspaces {
            for record in self.records(&workspace)? {
                if record.status == "approved" && !record.invoice.supplier.trim().is_empty() {
                    self.remember_supplier(
                        &workspace,
                        &record.invoice.identity_keys,
                        &record.invoice.supplier,
                    )?;
                    remembered += 1;
                }
            }
        }
        Ok(remembered)
    }
    /// Identifier -> the supplier name a person confirmed, for this workspace only.
    pub fn known_suppliers(
        &self,
        workspace: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        let db = self.db.lock().unwrap();
        let mut statement =
            db.prepare("SELECT key,supplier FROM supplier_profiles WHERE workspace=?1")?;
        let rows = statement
            .query_map([workspace], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }
    pub fn account_by_email(&self, email: &str) -> Result<Option<(String, String)>> {
        Ok(self
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT id,email FROM accounts WHERE email=?1",
                [email.to_lowercase().trim()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }
    pub fn email_verified(&self, account: &str) -> Result<bool> {
        Ok(self.db.lock().unwrap().query_row(
            "SELECT email_verified_at IS NOT NULL FROM accounts WHERE id=?1",
            [account],
            |r| r.get(0),
        )?)
    }
    /// Creates a single-use token and returns it once. Only its hash is stored. Returns `None`
    /// while a token of the same kind was issued for this account within the cooldown, so the
    /// endpoint cannot be used to flood a mailbox.
    pub fn issue_email_token(
        &self,
        account: &str,
        purpose: &str,
        email: &str,
        ttl_seconds: i64,
    ) -> Result<Option<String>> {
        let now = chrono::Utc::now().timestamp();
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute("DELETE FROM email_tokens WHERE expires<?1", [now])?;
        let recent: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM email_tokens WHERE account=?1 AND purpose=?2 AND created>?3)",
            params![account, purpose, now - EMAIL_TOKEN_COOLDOWN_SECONDS],
            |r| r.get(0),
        )?;
        if recent {
            return Ok(None);
        }
        tx.execute(
            "DELETE FROM email_tokens WHERE account=?1 AND purpose=?2",
            params![account, purpose],
        )?;
        let token = random_token();
        tx.execute(
            "INSERT INTO email_tokens VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                hash(token.as_bytes()),
                account,
                purpose,
                email,
                now,
                now + ttl_seconds
            ],
        )?;
        tx.commit()?;
        Ok(Some(token))
    }
    /// Looks up and deletes a token in one transaction, so it can be used only once.
    pub fn consume_email_token(
        &self,
        purpose: &str,
        token: &str,
    ) -> Result<Option<(String, String)>> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let found: Option<(String, String)> = tx
            .query_row(
                "SELECT account,email FROM email_tokens WHERE hash=?1 AND purpose=?2 AND expires>?3",
                params![
                    hash(token.as_bytes()),
                    purpose,
                    chrono::Utc::now().timestamp()
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if found.is_some() {
            tx.execute(
                "DELETE FROM email_tokens WHERE hash=?1",
                [hash(token.as_bytes())],
            )?;
        }
        tx.commit()?;
        Ok(found)
    }
    /// Marks the address verified only if it is still the address the token was sent to.
    pub fn mark_email_verified(&self, account: &str, email: &str) -> Result<bool> {
        let db = self.db.lock().unwrap();
        Ok(db.execute(
            "UPDATE accounts SET email_verified_at=COALESCE(email_verified_at,?3) WHERE id=?1 AND email=?2",
            params![account, email, now()],
        )? > 0)
    }
    /// Replaces the password, signs the account out everywhere and discards outstanding reset
    /// tokens. Following a reset link proves control of the mailbox, so the address counts as
    /// verified.
    pub fn reset_password(&self, account: &str, password: &str) -> Result<()> {
        let encoded = hash_password(password)?;
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute(
            "UPDATE accounts SET password=?2,email_verified_at=COALESCE(email_verified_at,?3) WHERE id=?1",
            params![account, encoded, now()],
        )?;
        tx.execute("DELETE FROM sessions WHERE account=?1", [account])?;
        tx.execute(
            "DELETE FROM email_tokens WHERE account=?1 AND purpose='reset'",
            [account],
        )?;
        tx.commit()?;
        Ok(())
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
    fn remembered_suppliers_are_per_workspace_and_the_latest_confirmation_wins() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let a = store
            .register("a@example.com", "correct horse battery staple", "A")
            .unwrap();
        let b = store
            .register("b@example.com", "correct horse battery staple", "B")
            .unwrap();
        let (a, b) = (
            store.workspace_for_user(&a).unwrap(),
            store.workspace_for_user(&b).unwrap(),
        );
        let keys = vec!["tax:111222333RT0001".to_string()];
        store
            .remember_supplier(&a, &keys, "Quartz Valve Corp.")
            .unwrap();
        let known = store.known_suppliers(&a).unwrap();
        assert_eq!(known["tax:111222333RT0001"], "Quartz Valve Corp.");
        assert_eq!(
            known["name:quartzvalvecorp"], "Quartz Valve Corp.",
            "the confirmed name is a key too"
        );
        assert!(store.known_suppliers(&b).unwrap().is_empty());

        store
            .remember_supplier(&a, &keys, "Quartz Valve Corporation")
            .unwrap();
        assert_eq!(
            store.known_suppliers(&a).unwrap()["tax:111222333RT0001"],
            "Quartz Valve Corporation"
        );
        store.remember_supplier(&a, &keys, "   ").unwrap();
        assert_eq!(
            store.known_suppliers(&a).unwrap().len(),
            3,
            "a blank name remembers nothing"
        );
    }
    #[test]
    fn earlier_approvals_are_remembered_once_and_only_when_nothing_is_remembered_yet() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let user = store
            .register("a@example.com", "correct horse battery staple", "A")
            .unwrap();
        let workspace = store.workspace_for_user(&user).unwrap();
        let id = store
            .insert_document(&workspace, "a.pdf", b"%PDF-approved", "Upload", None, None)
            .unwrap()
            .unwrap();
        let mut record = store.record(&workspace, &id).unwrap().unwrap();
        record.status = "approved".into();
        record.invoice.supplier = "Crest Fire Extinguishers Ltd.".into();
        store.commit_record(&workspace, &record, None).unwrap();
        assert!(store.known_suppliers(&workspace).unwrap().is_empty());

        assert_eq!(store.backfill_supplier_profiles().unwrap(), 1);
        assert_eq!(
            store.known_suppliers(&workspace).unwrap()["name:crestfireextinguishersltd"],
            "Crest Fire Extinguishers Ltd."
        );
        // Already populated: a later start does not rewrite anything.
        assert_eq!(store.backfill_supplier_profiles().unwrap(), 0);
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
