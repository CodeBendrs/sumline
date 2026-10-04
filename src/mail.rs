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
            "https://gmail.googleapis.com/gmail/v1/users/me/profile",
        )
        .await?
    } else {
        get_json(
            app,
            &tokens.access,
            "https://graph.microsoft.com/v1.0/me?$select=mail,userPrincipalName",
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
        tx.execute("INSERT INTO connectors(account,provider,email,tokens,last_error) VALUES (?1,?2,?3,?4,NULL) ON CONFLICT(account,provider) DO UPDATE SET email=excluded.email,tokens=excluded.tokens,last_sync=NULL,last_error=NULL",params![account,provider,email,app.store.seal_json(account,&tokens)?])?;
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

fn allowed_url(url: &str) -> Result<()> {
    let url = url::Url::parse(url)?;
    if url.scheme() != "https"
        || !["gmail.googleapis.com", "graph.microsoft.com"].contains(&url.host_str().unwrap_or(""))
        || url.username() != ""
        || url.password().is_some()
    {
        return Err(anyhow!("Unexpected email API URL"));
    }
    Ok(())
}

async fn get_json(app: &App, token: &str, url: &str) -> Result<Value> {
    allowed_url(url)?;
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
            _ => return Err(anyhow!("Email provider request failed")),
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
            app.store.db.lock().unwrap().execute(
                "UPDATE connectors SET last_error=NULL WHERE account=?1 AND provider=?2",
                params![account, provider],
            )?;
            app.store.event(
                account,
                "sync",
                &format!("Inbox checked. {count} new PDF attachments received."),
            )?;
        }
        Err(_) => {
            app.store.db.lock().unwrap().execute("UPDATE connectors SET last_error='Email check failed. Retry or reconnect your account.' WHERE account=?1 AND provider=?2",params![account,provider])?;
            app.store.event(
                account,
                "error",
                "Email check failed. Retry or reconnect your account.",
            )?;
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
    let since = previous
        .and_then(|v| DateTime::parse_from_rfc3339(&v).ok())
        .map(|v| v.with_timezone(&Utc) - Duration::days(1))
        .unwrap_or(Utc::now() - Duration::days(lookback_days()));
    let mut count = 0;
    if provider == "google" {
        let query = format!(
            "has:attachment filename:pdf after:{}",
            since.format("%Y/%m/%d")
        );
        let mut next = Some(format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages?maxResults=100&q={}",
            url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
        ));
        let mut pages = 0;
        while let Some(url) = next.take() {
            pages += 1;
            if pages > 30 {
                return Err(anyhow!(
                    "Mailbox scan limit reached. Retry to continue the backfill."
                ));
            }
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
                    &format!(
                        "https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}?format=full"
                    ),
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
                        get_json(app,&tokens.access,&format!("https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}/attachments/{attachment}")).await?
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
            next=batch["nextPageToken"].as_str().map(|token|format!("https://gmail.googleapis.com/gmail/v1/users/me/messages?maxResults=100&q={}&pageToken={}",url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>(),url::form_urlencoded::byte_serialize(token.as_bytes()).collect::<String>()));
        }
    } else {
        let filter = format!(
            "receivedDateTime ge {} and hasAttachments eq true",
            since.to_rfc3339()
        );
        let mut next = Some(format!(
            "https://graph.microsoft.com/v1.0/me/messages?$top=50&$select=id,subject,from,receivedDateTime&$filter={}",
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
                    "https://graph.microsoft.com/v1.0/me/messages/{escaped}/attachments?$select=id,name,size,isInline,contentType"
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
                        let content = get_json(app, &tokens.access, &format!("https://graph.microsoft.com/v1.0/me/messages/{escaped}/attachments/{attachment_id}")).await?;
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

pub async fn poll(app: App) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let rows = {
            let db = app.store.db.lock().unwrap();
            let mut stmt = db
                .prepare("SELECT account,provider FROM connectors")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap_or_default()
        };
        let mut checks = tokio::task::JoinSet::new();
        for (account, provider) in rows {
            if checks.len() >= 4 {
                let _ = checks.join_next().await;
            }
            let app = app.clone();
            checks.spawn(async move {
                let _ = sync(&app, &account, &provider).await;
            });
        }
        while checks.join_next().await.is_some() {}
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
            }),
            http: reqwest::Client::new(),
            queue,
            sync_locks: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashSet::new()),
            ),
            auth_attempts: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
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
        assert!(allowed_url("https://graph.microsoft.com/v1.0/me/messages").is_ok());
        assert!(allowed_url("https://evil.example/?token=1").is_err());
        assert!(allowed_url("http://graph.microsoft.com/v1.0/me").is_err());
        assert!(allowed_url("https://graph.microsoft.com@evil.example/").is_err());
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
