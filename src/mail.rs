use crate::{
    App,
    storage::{hash, now, random_token},
};
use anyhow::{Result, anyhow};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenResponse, TokenUrl,
    basic::BasicClient,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Serialize, Deserialize)]
pub struct Tokens {
    access: String,
    refresh: Option<String>,
    expires: i64,
    #[serde(default)]
    generation: String,
}

pub const GMAIL_API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
pub const GRAPH_API: &str = "https://graph.microsoft.com/v1.0";

/// A non-success answer from a provider, kept typed so callers can react to specific statuses
/// (a 404 from Gmail's history endpoint means "start over", not "failed").
#[derive(Debug)]
struct HttpStatus(u16);
impl std::fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Email provider request failed")
    }
}
impl std::error::Error for HttpStatus {}
fn has_status(error: &anyhow::Error, status: u16) -> bool {
    error
        .downcast_ref::<HttpStatus>()
        .is_some_and(|e| e.0 == status)
}

pub fn lookback_days() -> i64 {
    std::env::var("INVOICE_INITIAL_LOOKBACK_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30)
        .clamp(1, 90)
}

fn endpoints(provider: &str) -> Result<(&'static str, &'static str, &'static str)> {
    match provider {
        "google" => Ok((
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            "https://www.googleapis.com/auth/gmail.readonly",
        )),
        "microsoft" => Ok((
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            "Mail.Read",
        )),
        _ => Err(anyhow!("Unsupported email provider")),
    }
}

pub fn credentials(app: &App, provider: &str) -> Option<(String, String)> {
    let prefix = match provider {
        "google" => "GOOGLE",
        "microsoft" => "MICROSOFT",
        _ => return None,
    };
    let id = std::env::var(format!("{prefix}_CLIENT_ID")).ok()?;
    let secret = std::env::var(format!("{prefix}_CLIENT_SECRET")).ok()?;
    let _ = app;
    if id.trim().is_empty() || secret.trim().is_empty() {
        None
    } else {
        Some((id, secret))
    }
}

pub fn start(app: &App, account: &str, session: &str, provider: &str) -> Result<String> {
    let (id, secret) = credentials(app, provider)
        .ok_or_else(|| anyhow!("This email connection is not configured in the local preview."))?;
    let (auth, token, scope) = endpoints(provider)?;
    let client = BasicClient::new(ClientId::new(id))
        .set_client_secret(ClientSecret::new(secret))
        .set_auth_uri(AuthUrl::new(auth.into())?)
        .set_token_uri(TokenUrl::new(token.into())?)
        .set_redirect_uri(RedirectUrl::new(format!(
            "{}/api/oauth/{provider}/callback",
            app.config.origin
        ))?);
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let nonce = random_token();
    let mut request = client
        .authorize_url(|| CsrfToken::new(nonce.clone()))
        .add_scope(Scope::new(scope.into()))
        .set_pkce_challenge(challenge);
    if provider == "google" {
        request = request
            .add_extra_param("access_type", "offline")
            .add_extra_param("prompt", "consent");
    } else {
        request = request
            .add_scope(Scope::new("offline_access".into()))
            .add_scope(Scope::new("User.Read".into()));
    }
    let (url, _) = request.url();
    app.store.db.lock().unwrap().execute(
        "INSERT INTO oauth_states VALUES (?1,?2,?3,?4,?5,?6)",
        params![
            hash(nonce.as_bytes()),
            account,
            hash(session.as_bytes()),
            provider,
            app.store.seal(account, verifier.secret().as_bytes())?,
            Utc::now().timestamp() + 600
        ],
    )?;
    Ok(url.to_string())
}

pub async fn finish(
    app: &App,
    account: &str,
    session: &str,
    provider: &str,
    state: &str,
    code: &str,
) -> Result<()> {
    let verifier: Option<Vec<u8>> = {
        let db = app.store.db.lock().unwrap();
        let row=db.query_row("SELECT verifier FROM oauth_states WHERE hash=?1 AND account=?2 AND session_hash=?3 AND provider=?4 AND expires>?5",params![hash(state.as_bytes()),account,hash(session.as_bytes()),provider,Utc::now().timestamp()],|r|r.get(0)).optional()?;
        db.execute(
            "DELETE FROM oauth_states WHERE hash=?1",
            [hash(state.as_bytes())],
        )?;
        row
    };
    let verifier = String::from_utf8(app.store.unseal(
        account,
        &verifier.ok_or_else(|| anyhow!("Expired or invalid authorization request"))?,
    )?)?;
    let (id, secret) = credentials(app, provider).ok_or_else(|| anyhow!("Provider unavailable"))?;
    let (_, token, _) = endpoints(provider)?;
    let client = BasicClient::new(ClientId::new(id))
        .set_client_secret(ClientSecret::new(secret))
        .set_token_uri(TokenUrl::new(token.into())?)
        .set_redirect_uri(RedirectUrl::new(format!(
            "{}/api/oauth/{provider}/callback",
            app.config.origin
        ))?);
    let response = client
        .exchange_code(AuthorizationCode::new(code.into()))
        .set_pkce_verifier(PkceCodeVerifier::new(verifier))
        .request_async(&app.http)
        .await
        .map_err(|_| anyhow!("Email authorization failed"))?;
    let tokens = Tokens {
        generation: random_token(),
        access: response.access_token().secret().clone(),
        refresh: response.refresh_token().map(|v| v.secret().clone()),
        expires: Utc::now().timestamp()
            + response
                .expires_in()
                .map(|v| v.as_secs() as i64)
                .unwrap_or(3600),
    };
    if tokens.refresh.is_none() {
        return Err(anyhow!(
            "Offline email access was not granted. Reconnect and allow background access."
        ));
    }
    let profile = if provider == "google" {
        get_json(
            app,
            &tokens.access,
            &format!("{}/profile", app.config.gmail_api),
        )
        .await?
    } else {
        get_json(
            app,
            &tokens.access,
            &format!("{}/me?$select=mail,userPrincipalName", app.config.graph_api),
        )
        .await?
    };
    let email = profile["emailAddress"]
        .as_str()
        .or_else(|| profile["mail"].as_str())
        .or_else(|| profile["userPrincipalName"].as_str())
        .ok_or_else(|| anyhow!("Unable to identify the connected account"))?;
    {
        let mut db = app.store.db.lock().unwrap();
        let tx = db.transaction()?;
        tx.execute("INSERT INTO connectors(account,provider,email,tokens,last_error) VALUES (?1,?2,?3,?4,NULL) ON CONFLICT(account,provider) DO UPDATE SET email=excluded.email,tokens=excluded.tokens,last_sync=NULL,last_error=NULL,cursor=NULL,scan_state=NULL",params![account,provider,email,app.store.seal_json(account,&tokens)?])?;
        tx.execute(
            "DELETE FROM processed_messages WHERE account=?1 AND provider=?2",
            params![account, provider],
        )?;
        tx.commit()?;
    }
    app.store.event(
        account,
        "connection",
        "An existing email account was connected with read-only mail access.",
    )?;
    Ok(())
}

async fn access(app: &App, account: &str, provider: &str) -> Result<Tokens> {
    let encrypted: Vec<u8> = app.store.db.lock().unwrap().query_row(
        "SELECT tokens FROM connectors WHERE account=?1 AND provider=?2",
        params![account, provider],
        |r| r.get(0),
    )?;
    let mut tokens: Tokens = app.store.read_json(account, &encrypted)?;
    if tokens.expires > Utc::now().timestamp() + 90 {
        return Ok(tokens);
    }
    let (id, secret) = credentials(app, provider).ok_or_else(|| anyhow!("Provider unavailable"))?;
    let (_, token, _) = endpoints(provider)?;
    let client = BasicClient::new(ClientId::new(id))
        .set_client_secret(ClientSecret::new(secret))
        .set_token_uri(TokenUrl::new(token.into())?);
    let result = client
        .exchange_refresh_token(&RefreshToken::new(
            tokens
                .refresh
                .clone()
                .ok_or_else(|| anyhow!("Reconnect email account"))?,
        ))
        .request_async(&app.http)
        .await
        .map_err(|_| anyhow!("Reconnect email account"))?;
    tokens.access = result.access_token().secret().clone();
    if let Some(refresh) = result.refresh_token() {
        tokens.refresh = Some(refresh.secret().clone());
    }
    tokens.expires = Utc::now().timestamp()
        + result
            .expires_in()
            .map(|v| v.as_secs() as i64)
            .unwrap_or(3600);
    let changed = app.store.db.lock().unwrap().execute(
        "UPDATE connectors SET tokens=?1 WHERE account=?2 AND provider=?3 AND tokens=?4",
        params![
            app.store.seal_json(account, &tokens)?,
            account,
            provider,
            encrypted
        ],
    )?;
    if changed == 0 {
        return Err(anyhow!("Email connection was changed"));
    }
    Ok(tokens)
}

/// Tokens are only ever sent to the configured provider APIs, never to a URL a response named.
fn allowed_url(config: &crate::Config, url: &str) -> Result<()> {
    let url = url::Url::parse(url)?;
    let same_origin = |base: &String| {
        url::Url::parse(base).is_ok_and(|b| {
            b.scheme() == url.scheme()
                && b.host_str() == url.host_str()
                && b.port_or_known_default() == url.port_or_known_default()
        })
    };
    if !(same_origin(&config.gmail_api) || same_origin(&config.graph_api))
        || url.username() != ""
        || url.password().is_some()
    {
        return Err(anyhow!("Unexpected email API URL"));
    }
    Ok(())
}

async fn get_json(app: &App, token: &str, url: &str) -> Result<Value> {
    allowed_url(&app.config, url)?;
    let key = hash(token.as_bytes());
    let until: Option<i64> = app
        .store
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT until FROM provider_cooldowns WHERE token=?1",
            [&key],
            |r| r.get(0),
        )
        .optional()?;
    if until.is_some_and(|v| v > Utc::now().timestamp()) {
        return Err(anyhow!("Email provider is throttling requests"));
    }
    let mut attempt = 0;
    let mut response = loop {
        let response = app.http.get(url).bearer_auth(token).send().await;
        match response {
            Ok(response) if response.status().is_success() => break response,
            Ok(response)
                if (response.status().as_u16() == 429 || response.status().is_server_error())
                    && attempt < 3 =>
            {
                let delay = retry_delay(
                    response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok()),
                    attempt,
                    chrono::Utc::now(),
                );
                // Long throttles defer to the next scheduled check instead of occupying a worker.
                if delay > 60 {
                    let db = app.store.db.lock().unwrap();
                    db.execute(
                        "DELETE FROM provider_cooldowns WHERE until<=?1",
                        [Utc::now().timestamp()],
                    )?;
                    db.execute("INSERT INTO provider_cooldowns VALUES (?1,?2) ON CONFLICT(token) DO UPDATE SET until=excluded.until",params![key,Utc::now().timestamp().saturating_add(delay.min(i64::MAX as u64) as i64)])?;
                    return Err(anyhow!("Email provider is throttling requests"));
                }
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }
            Err(_) if attempt < 3 => {
                tokio::time::sleep(std::time::Duration::from_secs(1_u64 << attempt)).await;
            }
            Ok(response) => return Err(HttpStatus(response.status().as_u16()).into()),
            Err(_) => return Err(anyhow!("Email provider request failed")),
        }
        attempt += 1;
    };
    if response
        .content_length()
        .is_some_and(|v| v > 30 * 1024 * 1024)
    {
        return Err(anyhow!("Attachment exceeds the size limit"));
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if data.len() + chunk.len() > 30 * 1024 * 1024 {
            return Err(anyhow!("Email response exceeds the size limit"));
        }
        data.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&data)?)
}

fn retry_delay(header: Option<&str>, attempt: u32, now: chrono::DateTime<chrono::Utc>) -> u64 {
    let jitter = rand::random::<u64>() % (1_u64 << attempt.min(5)) + 1;
    let requested = header.and_then(|v| {
        v.parse::<u64>().ok().or_else(|| {
            chrono::DateTime::parse_from_rfc2822(v)
                .ok()
                .map(|time| (time.timestamp() - now.timestamp()).max(0) as u64)
        })
    });
    requested.unwrap_or(jitter).max(1)
}

fn parts(value: &Value, output: &mut Vec<Value>) {
    if value["filename"]
        .as_str()
        .is_some_and(|v| v.to_lowercase().ends_with(".pdf"))
    {
        output.push(value.clone());
    }
    if let Some(children) = value["parts"].as_array() {
        for child in children {
            parts(child, output);
        }
    }
}

fn seen(app: &App, account: &str, provider: &str, id: &str) -> Result<bool> {
    Ok(app.store.db.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM processed_messages WHERE account=?1 AND provider=?2 AND message_id=?3)",params![account,provider,id],|r|r.get(0))?)
}
fn mark_seen(app: &App, account: &str, provider: &str, id: &str) -> Result<()> {
    app.store.db.lock().unwrap().execute(
        "INSERT OR IGNORE INTO processed_messages VALUES (?1,?2,?3)",
        params![account, provider, id],
    )?;
    Ok(())
}

pub async fn sync(app: &App, account: &str, provider: &str) -> Result<usize> {
    let key = format!("{account}:{provider}");
    {
        let mut locks = app.sync_locks.lock().unwrap();
        if !locks.insert(key.clone()) {
            return Ok(0);
        }
    }
    let result = sync_inner(app, account, provider).await;
    app.sync_locks.lock().unwrap().remove(&key);
    match &result {
        Ok(count) => {
            // A check that finds nothing is not news: no activity entry, and no write unless a
            // previous failure is being cleared.
            app.store.db.lock().unwrap().execute(
                "UPDATE connectors SET last_error=NULL WHERE account=?1 AND provider=?2 AND last_error IS NOT NULL",
                params![account, provider],
            )?;
            if *count > 0 {
                app.store.event(
                    account,
                    "sync",
                    &format!("Inbox checked. {count} new PDF attachments received."),
                )?;
            }
        }
        Err(_) => {
            // Report a failure once when it starts, not at every retry while it lasts.
            let first = app.store.db.lock().unwrap().execute("UPDATE connectors SET last_error='Email check failed. Retry or reconnect your account.' WHERE account=?1 AND provider=?2 AND last_error IS NULL",params![account,provider])? > 0;
            if first {
                app.store.event(
                    account,
                    "error",
                    "Email check failed. Retry or reconnect your account.",
                )?;
            }
        }
    }
    result
}

async fn sync_inner(app: &App, account: &str, provider: &str) -> Result<usize> {
    let tokens = access(app, account, provider).await?;
    let timestamp = now();
    let previous: Option<String> = app.store.db.lock().unwrap().query_row(
        "SELECT last_sync FROM connectors WHERE account=?1 AND provider=?2",
        params![account, provider],
        |r| r.get(0),
    )?;
    // Gmail's date filter works in whole days, so it keeps a day of overlap; Outlook filters by
    // exact arrival time, so a couple of hours is plenty. Messages already seen are skipped
    // locally either way, so overlap costs one list call and nothing else.
    let overlap = if provider == "google" {
        Duration::days(1)
    } else {
        Duration::hours(2)
    };
    let since = previous
        .and_then(|v| DateTime::parse_from_rfc3339(&v).ok())
        .map(|v| v.with_timezone(&Utc) - overlap)
        .unwrap_or(Utc::now() - Duration::days(lookback_days()));
    let mut count = 0;
    if provider == "google" {
        count = sync_gmail(app, account, &tokens, since).await?;
    } else {
        let filter = format!(
            "receivedDateTime ge {} and hasAttachments eq true",
            since.to_rfc3339()
        );
        let mut next = Some(format!(
            "{}/me/messages?$top=50&$select=id,subject,from,receivedDateTime&$filter={}",
            app.config.graph_api,
            url::form_urlencoded::byte_serialize(filter.as_bytes()).collect::<String>()
        ));
        let mut pages = 0;
        while let Some(url) = next.take() {
            pages += 1;
            if pages > 60 {
                return Err(anyhow!(
                    "Mailbox scan limit reached. Retry to continue the backfill."
                ));
            }
            let batch = get_json(app, &tokens.access, &url).await?;
            for message in batch["value"].as_array().into_iter().flatten() {
                let id = message["id"]
                    .as_str()
                    .ok_or_else(|| anyhow!("Invalid message ID"))?;
                if seen(app, account, provider, id)? {
                    continue;
                }
                let escaped =
                    url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>();
                let mut attachment_url = Some(format!(
                    "{}/me/messages/{escaped}/attachments?$select=id,name,size,isInline,contentType",
                    app.config.graph_api
                ));
                while let Some(url) = attachment_url.take() {
                    let attachments = get_json(app, &tokens.access, &url).await?;
                    for attachment in attachments["value"].as_array().into_iter().flatten() {
                        let name = attachment["name"].as_str().unwrap_or("");
                        if !name.to_lowercase().ends_with(".pdf")
                            || attachment["isInline"].as_bool().unwrap_or(false)
                        {
                            continue;
                        }
                        if attachment["size"].as_u64().unwrap_or(0) > 20 * 1024 * 1024 {
                            return Err(anyhow!("PDF attachment exceeds the size limit"));
                        }
                        ensure_connected(app, account, provider, &tokens.generation)?;
                        let attachment_id = attachment["id"]
                            .as_str()
                            .ok_or_else(|| anyhow!("Invalid attachment ID"))?;
                        let attachment_id =
                            url::form_urlencoded::byte_serialize(attachment_id.as_bytes())
                                .collect::<String>();
                        let content = get_json(
                            app,
                            &tokens.access,
                            &format!(
                                "{}/me/messages/{escaped}/attachments/{attachment_id}",
                                app.config.graph_api
                            ),
                        )
                        .await?;
                        let bytes = base64::engine::general_purpose::STANDARD.decode(
                            content["contentBytes"]
                                .as_str()
                                .ok_or_else(|| anyhow!("Attachment data unavailable"))?,
                        )?;
                        ensure_connected(app, account, provider, &tokens.generation)?;
                        if app
                            .import(
                                account,
                                name,
                                &bytes,
                                "Outlook",
                                message["subject"].as_str().map(str::to_string),
                                message["from"]["emailAddress"]["address"]
                                    .as_str()
                                    .map(str::to_string),
                            )
                            .await?
                            .is_some()
                        {
                            count += 1;
                        }
                    }
                    attachment_url = attachments["@odata.nextLink"].as_str().map(str::to_string);
                }
                ensure_connected(app, account, provider, &tokens.generation)?;
                mark_seen(app, account, provider, id)?;
            }
            next = batch["@odata.nextLink"].as_str().map(str::to_string);
        }
    }
    ensure_connected(app, account, provider, &tokens.generation)?;
    app.store.db.lock().unwrap().execute(
        "UPDATE connectors SET last_sync=?1 WHERE account=?2 AND provider=?3",
        params![timestamp, account, provider],
    )?;
    Ok(count)
}

/// Pages of 100 messages one check will list before pausing; the rest resumes on the next check.
const MAX_PAGES_PER_RUN: u32 = 30;

/// An unfinished listing of the mailbox, saved so it resumes where it stopped rather than
/// starting again (a large first scan can span several checks).
#[derive(Serialize, Deserialize, Clone)]
struct ScanState {
    since: String,
    token: Option<String>,
    /// The mailbox's history position taken before listing began. It becomes the cursor once the
    /// listing completes, so mail that arrives during the scan is caught by the next check.
    baseline: String,
}

fn save_sync_state(
    app: &App,
    account: &str,
    generation: &str,
    cursor: Option<&str>,
    scan: Option<&ScanState>,
) -> Result<()> {
    ensure_connected(app, account, "google", generation)?;
    app.store.db.lock().unwrap().execute(
        "UPDATE connectors SET cursor=COALESCE(?1,cursor),scan_state=?2 WHERE account=?3 AND provider='google'",
        params![cursor, scan.map(serde_json::to_string).transpose()?, account],
    )?;
    Ok(())
}

fn load_sync_state(app: &App, account: &str) -> Result<(Option<String>, Option<ScanState>)> {
    let (cursor, scan): (Option<String>, Option<String>) = app.store.db.lock().unwrap().query_row(
        "SELECT cursor,scan_state FROM connectors WHERE account=?1 AND provider='google'",
        [account],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok((cursor, scan.and_then(|s| serde_json::from_str(&s).ok())))
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

struct Probe {
    arrived: bool,
    latest: String,
}

/// Asks Gmail whether any message was added since `cursor`. This is one two-unit call that
/// returns nothing when the mailbox is quiet. It does not say whether a new message has a PDF,
/// so a hit is followed by the usual filtered listing: only matching messages are ever fetched.
async fn probe_history(app: &App, token: &str, cursor: &str) -> Result<Probe> {
    let mut page: Option<String> = None;
    let mut latest = cursor.to_string();
    for _ in 0..10 {
        let mut url = format!(
            "{}/history?startHistoryId={}&historyTypes=messageAdded&maxResults=500",
            app.config.gmail_api,
            encode(cursor)
        );
        if let Some(page) = &page {
            url.push_str(&format!("&pageToken={}", encode(page)));
        }
        let batch = get_json(app, token, &url).await?;
        if let Some(id) = batch["historyId"].as_str() {
            latest = id.to_string();
        }
        let arrived = batch["history"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|record| {
                record["messagesAdded"]
                    .as_array()
                    .is_some_and(|added| !added.is_empty())
            });
        if arrived {
            return Ok(Probe { arrived, latest });
        }
        page = batch["nextPageToken"].as_str().map(str::to_string);
        if page.is_none() {
            return Ok(Probe {
                arrived: false,
                latest,
            });
        }
    }
    // Too many empty pages to be sure: look rather than risk missing something.
    Ok(Probe {
        arrived: true,
        latest,
    })
}

async fn sync_gmail(
    app: &App,
    account: &str,
    tokens: &Tokens,
    since: DateTime<Utc>,
) -> Result<usize> {
    let (cursor, unfinished) = load_sync_state(app, account)?;
    // An interrupted or paused listing carries on first.
    if let Some(state) = unfinished {
        return run_scan(app, account, tokens, state).await;
    }
    if let Some(cursor) = cursor {
        match probe_history(app, &tokens.access, &cursor).await {
            Ok(probe) if !probe.arrived => {
                if probe.latest != cursor {
                    save_sync_state(app, account, &tokens.generation, Some(&probe.latest), None)?;
                }
                return Ok(0);
            }
            Ok(probe) => {
                let state = ScanState {
                    since: since.to_rfc3339(),
                    token: None,
                    baseline: probe.latest,
                };
                return run_scan(app, account, tokens, state).await;
            }
            // Gmail keeps change history for about a week. An older cursor means starting over.
            Err(error) if has_status(&error, 404) => {}
            Err(error) => return Err(error),
        }
    }
    // First scan, or history expired: note the mailbox position before listing so nothing that
    // arrives meanwhile is lost.
    let profile = get_json(
        app,
        &tokens.access,
        &format!("{}/profile", app.config.gmail_api),
    )
    .await?;
    let baseline = profile["historyId"]
        .as_str()
        .map(str::to_string)
        .or_else(|| profile["historyId"].as_u64().map(|v| v.to_string()))
        .ok_or_else(|| anyhow!("Mailbox position unavailable"))?;
    let state = ScanState {
        since: since.to_rfc3339(),
        token: None,
        baseline,
    };
    run_scan(app, account, tokens, state).await
}

async fn run_scan(
    app: &App,
    account: &str,
    tokens: &Tokens,
    mut state: ScanState,
) -> Result<usize> {
    let provider = "google";
    let since = DateTime::parse_from_rfc3339(&state.since)
        .map(|v| v.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now() - Duration::days(lookback_days()));
    save_sync_state(app, account, &tokens.generation, None, Some(&state))?;
    let query = encode(&format!(
        "has:attachment filename:pdf after:{}",
        since.format("%Y/%m/%d")
    ));
    let mut count = 0;
    let mut pages = 0;
    loop {
        if pages >= MAX_PAGES_PER_RUN {
            // Pause here; the saved page token lets the next check carry on.
            return Ok(count);
        }
        pages += 1;
        let url = match &state.token {
            None => format!("{}/messages?maxResults=100&q={query}", app.config.gmail_api),
            Some(token) => format!(
                "{}/messages?maxResults=100&q={query}&pageToken={}",
                app.config.gmail_api,
                encode(token)
            ),
        };
        let batch = get_json(app, &tokens.access, &url).await?;
        for message in batch["messages"].as_array().into_iter().flatten() {
            let id = message["id"]
                .as_str()
                .ok_or_else(|| anyhow!("Invalid message ID"))?;
            if seen(app, account, provider, id)? {
                continue;
            }
            let message = get_json(
                app,
                &tokens.access,
                &format!("{}/messages/{id}?format=full", app.config.gmail_api),
            )
            .await?;
            let headers = message["payload"]["headers"].as_array();
            let header = |name: &str| {
                headers
                    .and_then(|h| {
                        h.iter().find(|v| {
                            v["name"]
                                .as_str()
                                .is_some_and(|n| n.eq_ignore_ascii_case(name))
                        })
                    })
                    .and_then(|v| v["value"].as_str())
                    .map(str::to_string)
            };
            let mut attachments = Vec::new();
            parts(&message["payload"], &mut attachments);
            for part in attachments {
                ensure_connected(app, account, provider, &tokens.generation)?;
                if part["body"]["size"].as_u64().unwrap_or(0) > 20 * 1024 * 1024 {
                    return Err(anyhow!("PDF attachment exceeds the size limit"));
                }
                let body = if let Some(attachment) = part["body"]["attachmentId"].as_str() {
                    get_json(
                        app,
                        &tokens.access,
                        &format!(
                            "{}/messages/{id}/attachments/{attachment}",
                            app.config.gmail_api
                        ),
                    )
                    .await?
                } else {
                    part["body"].clone()
                };
                let bytes = URL_SAFE_NO_PAD
                    .decode(body["data"].as_str().unwrap_or("").trim_end_matches('='))?;
                let name = part["filename"].as_str().unwrap_or("invoice.pdf");
                ensure_connected(app, account, provider, &tokens.generation)?;
                if app
                    .import(
                        account,
                        name,
                        &bytes,
                        "Gmail",
                        header("Subject"),
                        header("From"),
                    )
                    .await?
                    .is_some()
                {
                    count += 1;
                }
            }
            ensure_connected(app, account, provider, &tokens.generation)?;
            mark_seen(app, account, provider, id)?;
        }
        match batch["nextPageToken"].as_str() {
            Some(next) => {
                state.token = Some(next.to_string());
                save_sync_state(app, account, &tokens.generation, None, Some(&state))?;
            }
            None => break,
        }
    }
    save_sync_state(
        app,
        account,
        &tokens.generation,
        Some(&state.baseline),
        None,
    )?;
    Ok(count)
}

fn ensure_connected(app: &App, account: &str, provider: &str, generation: &str) -> Result<()> {
    let encrypted: Option<Vec<u8>> = app
        .store
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT tokens FROM connectors WHERE account=?1 AND provider=?2",
            params![account, provider],
            |r| r.get(0),
        )
        .optional()?;
    let current: Tokens = app.store.read_json(
        account,
        &encrypted.ok_or_else(|| anyhow!("Email connection was removed"))?,
    )?;
    if current.generation != generation {
        return Err(anyhow!("Email connection was changed"));
    }
    Ok(())
}

pub fn connection_list(app: &App, account: &str) -> Result<Value> {
    let db = app.store.db.lock().unwrap();
    let mut stmt =
        db.prepare("SELECT provider,email,last_sync,last_error FROM connectors WHERE account=?1")?;
    let rows=stmt.query_map([account],|r|Ok(json!({"provider":r.get::<_,String>(0)?,"email":r.get::<_,String>(1)?,"last_sync":r.get::<_,Option<String>>(2)?,"last_error":r.get::<_,Option<String>>(3)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
    Ok(
        json!({"connections":rows,"available":{"google":credentials(app,"google").is_some(),"microsoft":credentials(app,"microsoft").is_some()}}),
    )
}

/// Seconds between checks of one mailbox (`INVOICE_POLL_SECONDS`, 60 to 3600, default 300).
pub fn poll_interval() -> u64 {
    std::env::var("INVOICE_POLL_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300)
        .clamp(60, 3600)
}

/// A stable offset within half an interval, so mailboxes do not all fire at the same instant.
fn jitter_seconds(key: &str, interval: u64) -> u64 {
    u64::from_str_radix(&hash(key.as_bytes())[..8], 16).unwrap_or(0) % (interval / 2).max(1)
}

/// How long to wait after a check: the normal interval after success, doubling after each
/// consecutive failure up to an hour, so a broken connection is not retried hammer-fast.
fn next_delay(interval: u64, failures: u32) -> u64 {
    if failures == 0 {
        interval
    } else {
        (interval << failures.min(6)).clamp(interval, 3600.max(interval))
    }
}

/// Checks every connected mailbox on its own schedule rather than all at once. A short tick
/// only looks for mailboxes that are due; four checks run at a time.
pub async fn poll(app: App) {
    use std::{
        collections::{HashMap, HashSet},
        sync::{Arc, Mutex},
        time::{Duration as Wait, Instant},
    };
    let interval = poll_interval();
    // key -> (when it is next due, consecutive failures)
    let schedule: Arc<Mutex<HashMap<String, (Instant, u32)>>> = Arc::default();
    let permits = Arc::new(tokio::sync::Semaphore::new(4));
    let mut tick = tokio::time::interval(Wait::from_secs(10));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let rows: Vec<(String, String)> = {
            let db = app.store.db.lock().unwrap();
            let Ok(mut stmt) = db.prepare("SELECT account,provider FROM connectors") else {
                continue;
            };
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        };
        let now = Instant::now();
        let mut due = Vec::new();
        {
            let mut schedule = schedule.lock().unwrap();
            let live: HashSet<String> = rows.iter().map(|(a, p)| format!("{a}:{p}")).collect();
            schedule.retain(|key, _| live.contains(key));
            for (account, provider) in &rows {
                let key = format!("{account}:{provider}");
                let entry = schedule
                    .entry(key.clone())
                    .or_insert_with(|| (now + Wait::from_secs(jitter_seconds(&key, interval)), 0));
                if now >= entry.0 {
                    // Marked busy until the check reports back; a hung check is retried later.
                    entry.0 = now + Wait::from_secs(600);
                    due.push((account.clone(), provider.clone(), key));
                }
            }
        }
        for (account, provider, key) in due {
            let app = app.clone();
            let schedule = schedule.clone();
            let permits = permits.clone();
            tokio::spawn(async move {
                let _permit = permits.acquire_owned().await;
                let succeeded = sync(&app, &account, &provider).await.is_ok();
                if let Some(entry) = schedule.lock().unwrap().get_mut(&key) {
                    entry.1 = if succeeded { 0 } else { entry.1 + 1 };
                    entry.0 = Instant::now() + Wait::from_secs(next_delay(interval, entry.1));
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_retry_after_seconds_and_http_dates_are_respected() {
        let now = chrono::Utc::now();
        assert_eq!(retry_delay(Some("45"), 0, now), 45);
        assert_eq!(
            retry_delay(
                Some(&(now + chrono::Duration::seconds(120)).to_rfc2822()),
                0,
                now
            ),
            120
        );
        assert!((1..=8).contains(&retry_delay(None, 3, now)));
    }
    #[test]
    fn old_sync_stops_when_connection_changes_or_is_removed() {
        let directory = tempfile::tempdir().unwrap();
        let store = crate::storage::Store::open(directory.path()).unwrap();
        store
            .db
            .lock()
            .unwrap()
            .execute("INSERT INTO workspaces VALUES ('workspace','Test')", [])
            .unwrap();
        let queue = std::sync::Arc::new(tokio::sync::Notify::new());
        let app = App {
            store,
            config: std::sync::Arc::new(crate::Config {
                origin: "http://localhost:3030".into(),
                demo: false,
                samples: None,
                trusted_proxies: Vec::new(),
                gmail_api: GMAIL_API.into(),
                graph_api: GRAPH_API.into(),
            }),
            http: reqwest::Client::new(),
            queue,
            sync_locks: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashSet::new()),
            ),
            auth_attempts: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            mailer: crate::outbound::Mailer::Disabled,
        };
        let tokens = Tokens {
            access: "fake".into(),
            refresh: None,
            expires: 0,
            generation: "new".into(),
        };
        app.store.db.lock().unwrap().execute("INSERT INTO connectors(account,provider,email,tokens) VALUES ('workspace','google','test@example.test',?1)",[app.store.seal_json("workspace", &tokens).unwrap()]).unwrap();
        assert!(ensure_connected(&app, "workspace", "google", "new").is_ok());
        assert!(ensure_connected(&app, "workspace", "google", "old").is_err());
        app.store
            .db
            .lock()
            .unwrap()
            .execute("DELETE FROM connectors", [])
            .unwrap();
        assert!(ensure_connected(&app, "workspace", "google", "new").is_err());
    }
    #[test]
    fn pagination_cannot_send_tokens_to_another_server() {
        let config = crate::Config {
            origin: "http://localhost:3030".into(),
            demo: false,
            samples: None,
            trusted_proxies: Vec::new(),
            gmail_api: GMAIL_API.into(),
            graph_api: GRAPH_API.into(),
        };
        assert!(allowed_url(&config, "https://graph.microsoft.com/v1.0/me/messages").is_ok());
        assert!(
            allowed_url(
                &config,
                "https://gmail.googleapis.com/gmail/v1/users/me/messages?pageToken=x"
            )
            .is_ok()
        );
        assert!(allowed_url(&config, "https://evil.example/?token=1").is_err());
        assert!(allowed_url(&config, "http://graph.microsoft.com/v1.0/me").is_err());
        assert!(allowed_url(&config, "https://graph.microsoft.com@evil.example/").is_err());
        assert!(allowed_url(&config, "https://graph.microsoft.com.evil.example/").is_err());
    }
    #[test]
    fn all_pdf_attachments_in_a_single_email_are_separate_candidates() {
        let message = json!({"mimeType":"multipart/mixed","parts":[{"filename":"victoria.pdf","body":{"attachmentId":"one"}},{"mimeType":"multipart/related","parts":[{"filename":"downtown.pdf","body":{"attachmentId":"two"}},{"filename":"logo.png","body":{"attachmentId":"three"}}]}]});
        let mut pdfs = Vec::new();
        parts(&message, &mut pdfs);
        assert_eq!(pdfs.len(), 2);
        assert_eq!(pdfs[0]["filename"], "victoria.pdf");
        assert_eq!(pdfs[1]["filename"], "downtown.pdf");
    }
}

/// The sync code run against a fake Gmail that records every request, so the cost of each kind
/// of check is asserted, not assumed.
#[cfg(test)]
mod incremental {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Path, Query, State},
        http::{StatusCode, Uri},
        routing::get,
    };
    use std::{
        collections::{HashMap, HashSet},
        sync::{Arc, Mutex},
    };

    #[derive(Default)]
    struct Mailbox {
        calls: Mutex<Vec<String>>,
        queries: Mutex<Vec<String>>,
        history: Mutex<u64>,
        oldest_history: Mutex<u64>,
        arrivals: Mutex<Vec<(u64, String)>>,
        listing: Mutex<Vec<String>>,
        page_size: Mutex<usize>,
        fail_with: Mutex<Option<u16>>,
    }
    type Shared = Arc<Mailbox>;
    type Reply = Result<Json<Value>, (StatusCode, Json<Value>)>;

    impl Mailbox {
        /// A message arrives: the mailbox history moves on and the message becomes listable.
        fn deliver(&self, id: &str) {
            let mut history = self.history.lock().unwrap();
            *history += 1;
            self.arrivals
                .lock()
                .unwrap()
                .push((*history, id.to_string()));
            self.listing.lock().unwrap().push(id.to_string());
        }
        fn kinds(&self) -> Vec<&'static str> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|path| match path.as_str() {
                    "/profile" => "profile",
                    "/history" => "history",
                    "/messages" => "list",
                    p if p.contains("/attachments/") => "attachment",
                    _ => "get",
                })
                .collect()
        }
        fn forget_calls(&self) {
            self.calls.lock().unwrap().clear();
        }
    }

    fn pdf_for(id: &str) -> Vec<u8> {
        format!("%PDF-1.4\n% fake invoice {id}\n").into_bytes()
    }
    fn enter(state: &Shared, uri: &Uri) -> Result<(), (StatusCode, Json<Value>)> {
        state.calls.lock().unwrap().push(uri.path().to_string());
        match *state.fail_with.lock().unwrap() {
            Some(code) => Err((
                StatusCode::from_u16(code).unwrap(),
                Json(json!({"error":{"code":code}})),
            )),
            None => Ok(()),
        }
    }
    async fn profile(State(s): State<Shared>, uri: Uri) -> Reply {
        enter(&s, &uri)?;
        let history = s.history.lock().unwrap().to_string();
        Ok(Json(
            json!({"emailAddress":"t@example.test","historyId":history}),
        ))
    }
    async fn history(
        State(s): State<Shared>,
        uri: Uri,
        Query(q): Query<HashMap<String, String>>,
    ) -> Reply {
        enter(&s, &uri)?;
        let start: u64 = q["startHistoryId"].parse().unwrap();
        if start < *s.oldest_history.lock().unwrap() {
            return Err((StatusCode::NOT_FOUND, Json(json!({"error":{"code":404}}))));
        }
        let current = s.history.lock().unwrap().to_string();
        let added: Vec<Value> = s
            .arrivals
            .lock()
            .unwrap()
            .iter()
            .filter(|(h, _)| *h > start)
            .map(|(h, id)| json!({"id":h.to_string(),"messagesAdded":[{"message":{"id":id}}]}))
            .collect();
        Ok(Json(if added.is_empty() {
            json!({"historyId":current})
        } else {
            json!({"history":added,"historyId":current})
        }))
    }
    async fn list(
        State(s): State<Shared>,
        uri: Uri,
        Query(q): Query<HashMap<String, String>>,
    ) -> Reply {
        enter(&s, &uri)?;
        s.queries
            .lock()
            .unwrap()
            .push(q.get("q").cloned().unwrap_or_default());
        let ids = s.listing.lock().unwrap().clone();
        let size = match *s.page_size.lock().unwrap() {
            0 => 100,
            n => n,
        };
        let start: usize = q
            .get("pageToken")
            .and_then(|t| t.strip_prefix('p'))
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        let end = (start + size).min(ids.len());
        let messages: Vec<Value> = ids[start..end].iter().map(|id| json!({"id":id})).collect();
        let mut body = json!({ "messages": messages });
        if end < ids.len() {
            body["nextPageToken"] = json!(format!("p{end}"));
        }
        Ok(Json(body))
    }
    async fn message(State(s): State<Shared>, uri: Uri, Path(id): Path<String>) -> Reply {
        enter(&s, &uri)?;
        Ok(Json(
            json!({"id":id,"payload":{"mimeType":"multipart/mixed",
            "headers":[{"name":"Subject","value":"Invoice"},{"name":"From","value":"supplier@example.test"}],
            "parts":[{"filename":format!("{id}.pdf"),"body":{"attachmentId":format!("a-{id}"),"size":pdf_for(&id).len()}}]}}),
        ))
    }
    async fn attachment(
        State(s): State<Shared>,
        uri: Uri,
        Path((id, _)): Path<(String, String)>,
    ) -> Reply {
        enter(&s, &uri)?;
        Ok(Json(json!({"data": URL_SAFE_NO_PAD.encode(pdf_for(&id))})))
    }

    async fn setup() -> (crate::testkit::Fixture, Shared) {
        let state: Shared = Arc::default();
        *state.history.lock().unwrap() = 100;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/profile", get(profile))
            .route("/history", get(history))
            .route("/messages", get(list))
            .route("/messages/{id}", get(message))
            .route("/messages/{id}/attachments/{attachment}", get(attachment))
            .with_state(state.clone());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut fixture = crate::testkit::fixture(false);
        let mut config = (*fixture.app.config).clone();
        config.gmail_api = base;
        fixture.app.config = Arc::new(config);
        let tokens = Tokens {
            access: "fake".into(),
            refresh: None,
            expires: Utc::now().timestamp() + 3600,
            generation: "g".into(),
        };
        let db = fixture.app.store.db.lock().unwrap();
        db.execute("INSERT INTO workspaces VALUES ('workspace','Test')", [])
            .unwrap();
        db.execute(
            "INSERT INTO connectors(account,provider,email,tokens) VALUES ('workspace','google','t@example.test',?1)",
            [fixture.app.store.seal_json("workspace", &tokens).unwrap()],
        )
        .unwrap();
        drop(db);
        (fixture, state)
    }
    fn cursor(app: &App) -> Option<String> {
        load_sync_state(app, "workspace").unwrap().0
    }
    fn events(app: &App, kind: &str) -> i64 {
        app.store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM events WHERE kind=?1", [kind], |r| {
                r.get(0)
            })
            .unwrap()
    }
    fn imported(app: &App) -> usize {
        app.store.records("workspace").unwrap().len()
    }

    #[tokio::test]
    async fn after_the_first_scan_a_quiet_check_is_one_history_call_and_writes_no_activity() {
        let (f, mailbox) = setup().await;
        mailbox.deliver("m1");
        // The first check lists the mailbox once, remembering its position first.
        assert_eq!(sync(&f.app, "workspace", "google").await.unwrap(), 1);
        assert_eq!(mailbox.kinds(), ["profile", "list", "get", "attachment"]);
        assert!(mailbox.queries.lock().unwrap()[0].contains("has:attachment filename:pdf"));
        assert_eq!(cursor(&f.app).as_deref(), Some("101"));
        assert_eq!(events(&f.app, "sync"), 1);

        // Nothing has arrived: exactly one cheap call, nothing listed, nothing logged.
        mailbox.forget_calls();
        for _ in 0..5 {
            assert_eq!(sync(&f.app, "workspace", "google").await.unwrap(), 0);
        }
        assert_eq!(mailbox.kinds(), ["history"; 5]);
        assert_eq!(
            events(&f.app, "sync"),
            1,
            "empty checks leave no activity entries"
        );
        assert_eq!(imported(&f.app), 1);
    }

    #[tokio::test]
    async fn a_new_arrival_triggers_one_filtered_listing_and_only_the_new_message_is_fetched() {
        let (f, mailbox) = setup().await;
        mailbox.deliver("m1");
        sync(&f.app, "workspace", "google").await.unwrap();
        mailbox.deliver("m2");
        mailbox.forget_calls();

        assert_eq!(sync(&f.app, "workspace", "google").await.unwrap(), 1);
        assert_eq!(
            mailbox.kinds(),
            ["history", "list", "get", "attachment"],
            "m1 is not fetched again"
        );
        assert_eq!(cursor(&f.app).as_deref(), Some("102"));
        assert_eq!(imported(&f.app), 2);

        mailbox.forget_calls();
        assert_eq!(sync(&f.app, "workspace", "google").await.unwrap(), 0);
        assert_eq!(mailbox.kinds(), ["history"]);
    }

    #[tokio::test]
    async fn expired_history_starts_over_without_duplicating_anything() {
        let (f, mailbox) = setup().await;
        mailbox.deliver("m1");
        sync(&f.app, "workspace", "google").await.unwrap();
        // Gmail forgets the old position (about a week of history is kept).
        *mailbox.history.lock().unwrap() = 600;
        *mailbox.oldest_history.lock().unwrap() = 500;
        mailbox.forget_calls();

        assert_eq!(sync(&f.app, "workspace", "google").await.unwrap(), 0);
        assert_eq!(
            mailbox.kinds(),
            ["history", "profile", "list"],
            "a full listing, but no re-fetching"
        );
        assert_eq!(cursor(&f.app).as_deref(), Some("600"));
        assert_eq!(imported(&f.app), 1);
        assert_eq!(
            events(&f.app, "error"),
            0,
            "an expired cursor is not an error"
        );
    }

    #[tokio::test]
    async fn a_large_backlog_pauses_and_resumes_instead_of_failing_forever() {
        let (f, mailbox) = setup().await;
        *mailbox.page_size.lock().unwrap() = 1;
        for n in 0..35 {
            mailbox.deliver(&format!("m{n:02}"));
        }
        // One check lists as many pages as it is allowed and saves where it stopped.
        assert_eq!(sync(&f.app, "workspace", "google").await.unwrap(), 30);
        let (cursor_now, scan) = load_sync_state(&f.app, "workspace").unwrap();
        assert!(
            cursor_now.is_none(),
            "the cursor only moves once the scan is complete"
        );
        assert!(scan.is_some_and(|s| s.token.as_deref() == Some("p30")));
        let error: Option<String> = f
            .app
            .store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT last_error FROM connectors", [], |r| r.get(0))
            .unwrap();
        assert!(error.is_none(), "pausing is not a failure");

        // The next check carries on from there and finishes.
        assert_eq!(sync(&f.app, "workspace", "google").await.unwrap(), 5);
        let (cursor_now, scan) = load_sync_state(&f.app, "workspace").unwrap();
        assert_eq!(cursor_now.as_deref(), Some("135"));
        assert!(scan.is_none());
        assert_eq!(imported(&f.app), 35);
    }

    #[tokio::test]
    async fn a_failure_is_reported_once_and_clears_when_the_connection_recovers() {
        let (f, mailbox) = setup().await;
        mailbox.deliver("m1");
        sync(&f.app, "workspace", "google").await.unwrap();

        *mailbox.fail_with.lock().unwrap() = Some(403);
        for _ in 0..3 {
            assert!(sync(&f.app, "workspace", "google").await.is_err());
        }
        assert_eq!(
            events(&f.app, "error"),
            1,
            "once per failure streak, not per retry"
        );
        let error: Option<String> = f
            .app
            .store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT last_error FROM connectors", [], |r| r.get(0))
            .unwrap();
        assert!(error.is_some());

        *mailbox.fail_with.lock().unwrap() = None;
        assert_eq!(sync(&f.app, "workspace", "google").await.unwrap(), 0);
        let error: Option<String> = f
            .app
            .store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT last_error FROM connectors", [], |r| r.get(0))
            .unwrap();
        assert!(error.is_none());
    }

    #[test]
    fn mailboxes_are_spread_across_the_interval_and_failures_back_off() {
        let offsets: HashSet<u64> = (0..200)
            .map(|n| jitter_seconds(&format!("workspace{n}:google"), 300))
            .collect();
        assert!(offsets.iter().all(|o| *o < 150));
        assert!(offsets.len() > 50, "offsets differ between mailboxes");
        assert_eq!(
            jitter_seconds("a:google", 300),
            jitter_seconds("a:google", 300)
        );

        assert_eq!(next_delay(300, 0), 300);
        assert_eq!(next_delay(300, 1), 600);
        assert_eq!(next_delay(300, 3), 2400);
        assert_eq!(next_delay(300, 4), 3600, "capped at an hour");
        assert_eq!(next_delay(300, 40), 3600);
        assert_eq!(next_delay(3600, 5), 3600);
    }
}
