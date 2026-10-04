mod accounts;
mod backup;
mod branding;
mod catalog;
mod extraction;
mod items;
mod layout;
mod mail;
mod model;
mod outbound;
mod privacy;
mod reliability;
mod spreadsheet;
mod storage;
mod supplier;
#[cfg(test)]
mod testkit;
mod worker;

use anyhow::{Result, anyhow};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use storage::{Store, hash, now};

#[derive(Clone)]
pub struct Config {
    origin: String,
    demo: bool,
    samples: Option<PathBuf>,
    /// Peers whose `X-Forwarded-For` header is believed (a reverse proxy we run ourselves).
    trusted_proxies: Vec<std::net::IpAddr>,
    /// Mailbox API roots. Always the real providers outside tests, which point them at a fake.
    gmail_api: String,
    graph_api: String,
}
#[derive(Clone)]
pub struct App {
    pub store: Store,
    pub config: Arc<Config>,
    pub http: reqwest::Client,
    pub queue: Arc<tokio::sync::Notify>,
    pub sync_locks: Arc<Mutex<HashSet<String>>>,
    auth_attempts: Arc<Mutex<HashMap<String, (std::time::Instant, u32)>>>,
    mailer: outbound::Mailer,
}

impl App {
    pub async fn import(
        &self,
        workspace: &str,
        name: &str,
        bytes: &[u8],
        source: &str,
        subject: Option<String>,
        sender: Option<String>,
    ) -> Result<Option<String>> {
        if bytes.len() > 20 * 1024 * 1024 || !bytes.starts_with(b"%PDF-") {
            return Err(anyhow!("Only PDF files up to 20 MB are supported."));
        }
        let name = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("invoice.pdf")
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect::<String>();
        let id = self
            .store
            .insert_document(workspace, &name, bytes, source, subject, sender)?;
        if let Some(id) = &id {
            let _ = id;
            self.queue.notify_waiters();
        }
        Ok(id)
    }
}

#[derive(Debug)]
struct ApiError(StatusCode, String);
impl From<reliability::Conflict> for ApiError {
    fn from(error: reliability::Conflict) -> Self {
        Self(StatusCode::CONFLICT, error.0)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        if let Some(conflict) = error.downcast_ref::<reliability::Conflict>() {
            return Self(StatusCode::CONFLICT, conflict.0.clone());
        }
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The request could not be completed. Please try again.".into(),
        )
    }
}
impl From<rusqlite::Error> for ApiError {
    fn from(_: rusqlite::Error) -> Self {
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The saved data could not be accessed.".into(),
        )
    }
}
type ApiResult<T> = std::result::Result<T, ApiError>;
fn bad(message: &str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}
fn missing() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "Invoice not found.".into())
}

struct Context {
    user: String,
    workspace: String,
    csrf: String,
    role: String,
    session: String,
}
fn cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|item| item.trim().strip_prefix("it_session=").map(str::to_string))
}
fn auth(app: &App, headers: &HeaderMap) -> ApiResult<Context> {
    let session = cookie(headers)
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "Sign in to continue.".into()))?;
    let (user, workspace, csrf, role) = app.store.session(&session)?.ok_or_else(|| {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "Your session has expired. Sign in again.".into(),
        )
    })?;
    Ok(Context {
        user,
        workspace,
        csrf,
        role,
        session,
    })
}
fn mutation(app: &App, headers: &HeaderMap) -> ApiResult<Context> {
    let context = auth(app, headers)?;
    if headers.get("x-csrf-token").and_then(|v| v.to_str().ok()) != Some(&context.csrf)
        || headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(&app.config.origin)
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "The request could not be verified. Refresh and try again.".into(),
        ));
    }
    Ok(context)
}
fn owner(context: &Context) -> ApiResult<()> {
    if context.role != "owner" {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only workspace owners can change this setting.".into(),
        ))
    } else {
        Ok(())
    }
}
fn login_response(app: &App, user: &str) -> ApiResult<Response> {
    let (session, csrf) = app.store.add_session(user)?;
    let secure = if app.config.origin.starts_with("https:") {
        "; Secure"
    } else {
        ""
    };
    Ok((
        [(
            header::SET_COOKIE,
            format!("it_session={session}; HttpOnly; SameSite=Lax; Path=/; Max-Age=43200{secure}"),
        )],
        Json(json!({"csrf":csrf})),
    )
        .into_response())
}

/// The address rate limits are counted against. Behind a reverse proxy every connection comes
/// from the proxy, so the forwarded client address is used, but only when the connection really
/// is from a configured proxy: anyone else could send that header themselves.
fn client_key(
    trusted: &[std::net::IpAddr],
    peer: Option<std::net::IpAddr>,
    forwarded: Option<&str>,
) -> String {
    let Some(peer) = peer else {
        return "local-test".into();
    };
    if trusted.contains(&peer)
        && let Some(client) = forwarded
            .and_then(|v| v.rsplit(',').next())
            .and_then(|v| v.trim().parse::<std::net::IpAddr>().ok())
    {
        return client.to_string();
    }
    peer.to_string()
}

async fn security(State(app): State<App>, request: axum::extract::Request, next: Next) -> Response {
    let expected = app
        .config
        .origin
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    // Container and uptime probes reach the server by a local name, not the public one.
    if request.uri().path() != "/healthz"
        && request
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            != Some(expected)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if request.uri().path().starts_with("/api/auth/") && request.method() == "POST" {
        if request
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            != Some(&app.config.origin)
        {
            return StatusCode::FORBIDDEN.into_response();
        }
        let key = client_key(
            &app.config.trusted_proxies,
            request
                .extensions()
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|v| v.0.ip()),
            request
                .headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok()),
        );
        let limited = {
            let mut attempts = app.auth_attempts.lock().unwrap();
            if attempts.len() > 10_000 {
                attempts.retain(|_, entry| entry.0.elapsed().as_secs() <= 60);
            }
            let entry = attempts
                .entry(key)
                .or_insert((std::time::Instant::now(), 0));
            if entry.0.elapsed().as_secs() > 60 {
                *entry = (std::time::Instant::now(), 0);
            }
            entry.1 += 1;
            entry.1 > 15
        };
        if limited {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error":"Too many sign-in attempts. Wait a minute and try again."})),
            )
                .into_response();
        }
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("referrer-policy", "no-referrer".parse().unwrap());
    headers.insert("x-frame-options", "DENY".parse().unwrap());
    headers.insert("content-security-policy","default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'".parse().unwrap());
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

#[derive(Deserialize)]
struct Credentials {
    email: String,
    password: String,
    store_name: Option<String>,
    consent: Option<bool>,
    policy_version: Option<String>,
}
async fn register(State(app): State<App>, Json(body): Json<Credentials>) -> ApiResult<Response> {
    if body.consent != Some(true) || body.policy_version.as_deref() != Some(privacy::POLICY_VERSION)
    {
        return Err(bad(
            "Read and accept the current privacy notice before creating an account.",
        ));
    }
    if !body.email.contains('@')
        || body.email.len() > 254
        || !(12..=128).contains(&body.password.len())
    {
        return Err(bad(
            "Use a valid email and a password of 12 to 128 characters.",
        ));
    }
    let name = body.store_name.as_deref().unwrap_or("My store").trim();
    if name.is_empty() || name.len() > 100 {
        return Err(bad("Enter a store name of up to 100 characters."));
    }
    let store = app.store.clone();
    let email = body.email.clone();
    let password = body.password.clone();
    let name = name.to_string();
    let user = tokio::task::spawn_blocking(move || store.register(&email, &password, &name))
        .await
        .map_err(|_| bad("Account creation failed."))?
        .map_err(|_| {
            bad("Account could not be created. Try signing in if you already have an account.")
        })?;
    let workspace = app.store.workspace_for_user(&user)?;
    app.store.consent(&user, &workspace, "account_processing")?;
    let verification = app.clone();
    let address = body.email.trim().to_lowercase();
    let account = user.clone();
    tokio::spawn(async move {
        accounts::send_verification(&verification, &account, &address).await;
    });
    login_response(&app, &user)
}
async fn login(State(app): State<App>, Json(body): Json<Credentials>) -> ApiResult<Response> {
    if body.email.len() > 254 || body.password.len() > 128 {
        return Err(bad("Email or password is incorrect."));
    }
    let store = app.store.clone();
    let user = tokio::task::spawn_blocking(move || store.login(&body.email, &body.password))
        .await
        .map_err(|_| bad("Sign-in failed."))??
        .ok_or_else(|| {
            ApiError(
                StatusCode::UNAUTHORIZED,
                "Email or password is incorrect.".into(),
            )
        })?;
    login_response(&app, &user)
}
async fn healthz(State(app): State<App>) -> Response {
    let healthy = app
        .store
        .db
        .lock()
        .unwrap()
        .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
        .is_ok();
    if healthy {
        (StatusCode::OK, Json(json!({"ok":true}))).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"ok":false}))).into_response()
    }
}
async fn logout(State(app): State<App>, headers: HeaderMap) -> ApiResult<Response> {
    let context = mutation(&app, &headers)?;
    app.store.db.lock().unwrap().execute(
        "DELETE FROM sessions WHERE hash=?1",
        [hash(context.session.as_bytes())],
    )?;
    Ok((
        [(
            header::SET_COOKIE,
            "it_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0",
        )],
        Json(json!({"ok":true})),
    )
        .into_response())
}
fn sample_workspace(app: &App, workspace: &str) -> Result<bool> {
    Ok(app.store.db.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM memberships m JOIN accounts a ON a.id=m.user WHERE m.workspace=?1 AND m.role='owner' AND a.email='preview@invoice-tracker.local')",[workspace],|r|r.get(0))?)
}
async fn me(State(app): State<App>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let context = auth(&app, &headers)?;
    let user = app.store.account(&context.user)?;
    let workspaces = {
        let db = app.store.db.lock().unwrap();
        let mut stmt=db.prepare("SELECT w.id,w.name,m.role FROM workspaces w JOIN memberships m ON m.workspace=w.id WHERE m.user=?1")?;
        stmt.query_map([&context.user],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"role":r.get::<_,String>(2)?})))?.collect::<std::result::Result<Vec<_>,_>>()?
    };
    Ok(Json(
        json!({"user":user,"workspace":context.workspace,"workspaces":workspaces,"role":context.role,"csrf":context.csrf,"stores":app.store.stores(&context.workspace)?,"demo":app.config.demo,"is_sample":sample_workspace(&app,&context.workspace)?,"initial_lookback_days":mail::lookback_days(),"policy_version":privacy::POLICY_VERSION,"mail_enabled":app.mailer.enabled(),"currencies":model::SUPPORTED_CURRENCIES}),
    ))
}
async fn demo(State(app): State<App>) -> ApiResult<Response> {
    if !app.config.demo {
        return Err(missing());
    }
    let email = "preview@invoice-tracker.local";
    let user: Option<String> = app
        .store
        .db
        .lock()
        .unwrap()
        .query_row("SELECT id FROM accounts WHERE email=?1", [email], |r| {
            r.get(0)
        })
        .optional()?;
    let user = if let Some(user) = user {
        user
    } else {
        let password = storage::random_token();
        app.store.register(email, &password, "Popeye's Victoria")?
    };
    let workspace = app.store.workspace_for_user(&user)?;
    {
        let db = app.store.db.lock().unwrap();
        db.execute(
            "UPDATE stores SET identifiers='[\"2500 Douglas\"]' WHERE workspace=?1",
            [&workspace],
        )?;
    }
    app.store.consent(&user, &workspace, "account_processing")?;
    if app.store.records(&workspace)?.is_empty()
        && let Some(directory) = &app.config.samples
    {
        for entry in std::fs::read_dir(directory)
            .map_err(|_| bad("Sample invoices are not available."))?
            .flatten()
        {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|v| v.eq_ignore_ascii_case("pdf"))
            {
                let data =
                    std::fs::read(&path).map_err(|_| bad("Sample invoice could not be read."))?;
                app.import(
                    &workspace,
                    &path.file_name().unwrap_or_default().to_string_lossy(),
                    &data,
                    "Sample",
                    None,
                    None,
                )
                .await?;
            }
        }
    }
    login_response(&app, &user)
}
#[derive(Deserialize, Default)]
struct InvoiceQuery {
    page: Option<usize>,
    view: Option<String>,
    store: Option<String>,
    search: Option<String>,
    #[serde(flatten)]
    filters: catalog::Filters,
}
fn validate_filters(filters: &catalog::Filters) -> ApiResult<()> {
    if filters.supplier.len() > 200
        || ![
            "",
            "posted",
            "ready",
            "approved",
            "review",
            "processing",
            "excluded",
            "bundle",
        ]
        .contains(&filters.status.as_str())
        || ![
            "",
            "invoice_newest",
            "invoice_oldest",
            "received_newest",
            "supplier_az",
            "total_high",
            "total_low",
        ]
        .contains(&filters.sort.as_str())
        || filters.currency.len() > 3
    {
        return Err(bad("Choose valid filters."));
    }
    for date in [&filters.from, &filters.to] {
        if !date.is_empty()
            && chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .ok()
                .is_none_or(|d| d.format("%Y-%m-%d").to_string() != *date)
        {
            return Err(bad("Choose valid dates."));
        }
    }
    if !filters.from.is_empty() && !filters.to.is_empty() && filters.from > filters.to {
        return Err(bad("The start date must be before the end date."));
    }
    Ok(())
}
async fn records(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<InvoiceQuery>,
) -> ApiResult<Json<Value>> {
    let c = auth(&app, &headers)?;
    let selected = query.store.unwrap_or_default();
    if !selected.is_empty()
        && selected != "unassigned"
        && !app.store.store_belongs(&c.workspace, &selected)?
    {
        return Err(missing());
    }
    let view = query.view.unwrap_or("archive".into());
    if !["invoices", "review", "archive"].contains(&view.as_str()) {
        return Err(bad("Choose an invoice view."));
    }
    let search = query.search.unwrap_or_default();
    if search.len() > 200 {
        return Err(bad("Use a shorter search."));
    }
    let filters = query.filters;
    validate_filters(&filters)?;
    let page = query.page.unwrap_or(1).clamp(1, 1_000_000);
    let store = app.store.clone();
    Ok(Json(
        tokio::task::spawn_blocking(move || {
            store.filtered_page(&c.workspace, &view, &selected, &search, page, &filters)
        })
        .await
        .map_err(|_| anyhow!("Listing failed"))??,
    ))
}
async fn invoice(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let c = auth(&app, &headers)?;
    Ok(Json(
        json!({"invoice":app.store.record(&c.workspace,&id)?.ok_or_else(missing)?}),
    ))
}
async fn upload(
    State(app): State<App>,
    headers: HeaderMap,
    mut files: Multipart,
) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    let mut result = Vec::new();
    let mut count = 0;
    while let Some(field) = files
        .next_field()
        .await
        .map_err(|_| bad("Could not read the upload."))?
    {
        count += 1;
        if count > 20 {
            return Err(bad("Upload up to 20 PDFs at once."));
        }
        let name = field.file_name().unwrap_or("invoice.pdf").to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|_| bad("The upload exceeds the file size limit."))?;
        let id = app
            .import(&c.workspace, &name, &bytes, "Upload", None, None)
            .await
            .map_err(|_| bad("Upload PDF files up to 20 MB each."))?;
        result.push(json!({"file_name":name,"id":id,"duplicate":id.is_none()}));
    }
    Ok(Json(json!({"files":result})))
}

#[derive(Deserialize, serde::Serialize)]
struct Review {
    revision: Option<i64>,
    supplier: String,
    invoice_number: String,
    invoice_date: String,
    due_date: Option<String>,
    currency: String,
    subtotal: String,
    tax: String,
    total: String,
    balance: Option<String>,
    po_number: Option<String>,
    store_id: String,
}
async fn reread_review(State(app): State<App>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    let count = app.store.reread_review(&c.workspace)?;
    if count > 0 {
        app.queue.notify_waiters();
    }
    Ok(Json(json!({"count":count})))
}
async fn approve(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Review>,
) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    let intent = reliability::Intent::from_headers(
        &headers,
        &c.user,
        &id,
        &serde_json::to_vec(&body).map_err(anyhow::Error::from)?,
    )
    .map_err(|_| bad("Use an idempotency key of 16 to 128 letters, numbers, or hyphens."))?;
    if let Some(intent) = &intent
        && let Some(record) = app.store.replay(&c.workspace, intent)?
    {
        return Ok(Json(json!({"invoice":record})));
    }
    let mut record = app.store.record(&c.workspace, &id)?.ok_or_else(missing)?;
    if body.revision.is_some_and(|v| v != record.revision) {
        return Err(reliability::Conflict(
            "This invoice changed. Reopen it to see the latest version before saving.".into(),
        )
        .into());
    }
    if record.status == "processing" || record.status == "bundle" {
        return Err(bad(
            "Wait for processing to finish and review each individual invoice.",
        ));
    }
    if !app.store.store_belongs(&c.workspace, &body.store_id)? {
        return Err(bad("Choose a store in this workspace."));
    }
    if body.supplier.trim().is_empty()
        || body.supplier.len() > 200
        || body.invoice_number.trim().is_empty()
        || body.invoice_number.len() > 100
    {
        return Err(bad("Supplier and invoice number are required."));
    }
    if chrono::NaiveDate::parse_from_str(&body.invoice_date, "%Y-%m-%d").is_err()
        || body.due_date.as_deref().is_some_and(|v| {
            !v.is_empty() && chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").is_err()
        })
    {
        return Err(bad("Enter valid dates."));
    }
    let currency = body.currency.trim().to_uppercase();
    if !model::SUPPORTED_CURRENCIES.contains(&currency.as_str()) {
        return Err(bad("Choose CAD or USD."));
    }
    let po_number = body
        .po_number
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    if po_number.as_ref().is_some_and(|v| v.chars().count() > 100) {
        return Err(bad("The PO number must be 100 characters or fewer."));
    }
    let subtotal =
        model::parse_money(&body.subtotal).ok_or_else(|| bad("Enter a valid subtotal."))?;
    let tax = model::parse_money(&body.tax).ok_or_else(|| bad("Enter a valid tax amount."))?;
    let total = model::parse_money(&body.total).ok_or_else(|| bad("Enter a valid total."))?;
    if (subtotal + tax - total).abs() > 1 {
        return Err(bad(
            "Subtotal plus tax must match the total. Check any discounts or shipping.",
        ));
    }
    let duplicate = app.store.records(&c.workspace)?.iter().any(|other| {
        other.id != id
            && other.store_id.as_deref() == Some(&body.store_id)
            && other
                .invoice
                .invoice_number
                .eq_ignore_ascii_case(body.invoice_number.trim())
            && other
                .invoice
                .supplier
                .eq_ignore_ascii_case(body.supplier.trim())
            && ["ready", "approved"].contains(&other.status.as_str())
    });
    if duplicate {
        return Err(bad(
            "This store already has this supplier's invoice number in the spreadsheet. Resolve the duplicate first.",
        ));
    }
    let invoice = &mut record.invoice;
    invoice.supplier = body.supplier.trim().into();
    invoice.invoice_number = body.invoice_number.trim().into();
    invoice.invoice_date = Some(body.invoice_date);
    invoice.due_date = body.due_date.filter(|v| !v.is_empty());
    invoice.currency = Some(currency);
    invoice.subtotal_cents = Some(subtotal);
    invoice.tax_cents = Some(tax);
    invoice.total_cents = Some(total);
    invoice.balance_cents = body
        .balance
        .as_deref()
        .filter(|v| !v.is_empty())
        .map(|value| model::parse_money(value).ok_or_else(|| bad("Enter a valid balance.")))
        .transpose()?;
    invoice.po_number = po_number;
    invoice.warnings.clear();
    record.store_id = Some(body.store_id);
    record.status = "approved".into();
    let record = app
        .store
        .commit_record(&c.workspace, &record, intent.as_ref())?;
    // The person has confirmed who issued this invoice; recognise that supplier next time.
    // This is a convenience layered on a saved approval, so a failure here must not undo it.
    if app
        .store
        .remember_supplier(
            &c.workspace,
            &record.invoice.identity_keys,
            &record.invoice.supplier,
        )
        .is_err()
    {
        eprintln!("A confirmed supplier could not be remembered.");
    }
    Ok(Json(json!({"invoice":record})))
}
async fn exclude(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    let mut record = app.store.record(&c.workspace, &id)?.ok_or_else(missing)?;
    if record.status == "processing" {
        return Err(bad("Wait for processing to finish."));
    }
    record.status = "excluded".into();
    app.store.update_record(&c.workspace, &record)?;
    Ok(Json(json!({"ok":true})))
}
async fn delete_invoice(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    let record = app.store.record(&c.workspace, &id)?.ok_or_else(missing)?;
    if record.status == "processing" {
        return Err(bad("Wait for processing to finish."));
    }
    app.store.delete_record(&c.workspace, &record)?;
    Ok(Json(json!({"ok":true})))
}
async fn document(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let c = auth(&app, &headers)?;
    let record = app.store.record(&c.workspace, &id)?.ok_or_else(missing)?;
    let data = app.store.document(&c.workspace, &id)?.ok_or_else(missing)?;
    let name = record
        .file_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || ['.', '-', '_', ' '].contains(c))
        .collect::<String>();
    Ok((
        [
            (header::CONTENT_TYPE, "application/pdf".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        data,
    )
        .into_response())
}
#[derive(Deserialize)]
struct ExportQuery {
    store: Option<String>,
    search: Option<String>,
    #[serde(flatten)]
    filters: catalog::Filters,
}
async fn export(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<ExportQuery>,
) -> ApiResult<Response> {
    let c = auth(&app, &headers)?;
    let store = app.store.clone();
    let workspace = c.workspace.clone();
    let origin = app.config.origin.clone();
    let selected = query.store.filter(|v| !v.is_empty());
    let filters = query.filters;
    validate_filters(&filters)?;
    let search = query.search.unwrap_or_default();
    if search.len() > 200 {
        return Err(bad("Use a shorter search."));
    }
    if let Some(id) = &selected
        && id != "unassigned"
        && !app.store.store_belongs(&workspace, id)?
    {
        return Err(missing());
    }
    let data = tokio::task::spawn_blocking(move || {
        spreadsheet::filtered_workbook(
            &store,
            &workspace,
            selected.as_deref(),
            &filters,
            &search,
            &origin,
        )
    })
    .await
    .map_err(|_| anyhow!("Export task failed"))??;

    Ok((
        [
            (
                header::CONTENT_TYPE,
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"Invoices.xlsx\"",
            ),
        ],
        data,
    )
        .into_response())
}
#[derive(Deserialize)]
struct StoreInput {
    name: String,
    identifiers: Vec<String>,
}
async fn add_store(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<StoreInput>,
) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    owner(&c)?;
    if body.name.trim().is_empty()
        || body.name.len() > 100
        || body.identifiers.len() > 10
        || body
            .identifiers
            .iter()
            .any(|v| v.len() < 8 || v.len() > 200)
    {
        return Err(bad(
            "Use a store name and specific invoice identifiers of 8 to 200 characters, such as a street address.",
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    app.store.db.lock().unwrap().execute(
        "INSERT INTO stores VALUES (?1,?2,?3,?4)",
        params![
            id,
            c.workspace,
            body.name,
            serde_json::to_string(&body.identifiers).unwrap()
        ],
    )?;
    Ok(Json(json!({"stores":app.store.stores(&c.workspace)?})))
}
async fn update_store(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<StoreInput>,
) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    owner(&c)?;
    if !app.store.store_belongs(&c.workspace, &id)? {
        return Err(missing());
    }
    if body.name.trim().is_empty()
        || body.name.len() > 100
        || body.identifiers.len() > 10
        || body
            .identifiers
            .iter()
            .any(|v| v.len() < 8 || v.len() > 200)
    {
        return Err(bad(
            "Use specific invoice identifiers of 8 to 200 characters, such as a street address.",
        ));
    }
    app.store.db.lock().unwrap().execute(
        "UPDATE stores SET name=?1,identifiers=?2 WHERE id=?3 AND workspace=?4",
        params![
            body.name,
            serde_json::to_string(&body.identifiers).unwrap(),
            id,
            c.workspace
        ],
    )?;
    Ok(Json(json!({"stores":app.store.stores(&c.workspace)?})))
}
async fn connections(State(app): State<App>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let c = auth(&app, &headers)?;
    Ok(Json(mail::connection_list(&app, &c.workspace)?))
}
#[derive(Deserialize)]
struct MailConsent {
    consent: bool,
    policy_version: String,
}
async fn connect(
    State(app): State<App>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Json(body): Json<MailConsent>,
) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    owner(&c)?;
    if !body.consent || body.policy_version != privacy::POLICY_VERSION {
        return Err(bad("Review and consent to email processing first."));
    }
    if sample_workspace(&app, &c.workspace)? {
        return Err(bad("Create your own workspace before connecting email."));
    }
    let url = mail::start(&app, &c.workspace, &c.session, &provider).map_err(|_| {
        bad("Email connection is currently unavailable. You can still upload PDFs.")
    })?;
    // This records the explicit purpose choice; OAuth grant is recorded separately after callback.
    app.store.consent(
        &c.user,
        &c.workspace,
        &format!("{provider}_email_processing"),
    )?;
    Ok(Json(json!({"url":url})))
}
async fn callback(
    State(app): State<App>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult<Redirect> {
    let c = auth(&app, &headers)?;
    owner(&c)?;
    let success = match (query.get("state"), query.get("code")) {
        (Some(state), Some(code)) => {
            mail::finish(&app, &c.workspace, &c.session, &provider, state, code)
                .await
                .is_ok()
        }
        _ => false,
    };
    if success {
        app.store
            .consent(&c.user, &c.workspace, &format!("{provider}_oauth_grant"))?;
        let cloned = app.clone();
        let account = c.workspace.clone();
        let provider = provider.clone();
        tokio::spawn(async move {
            let _ = mail::sync(&cloned, &account, &provider).await;
        });
    }
    Ok(Redirect::to(if success {
        "/?connection=connected"
    } else {
        "/?connection=failed"
    }))
}
async fn disconnect(
    State(app): State<App>,
    headers: HeaderMap,
    Path(provider): Path<String>,
) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    owner(&c)?;
    {
        let db = app.store.db.lock().unwrap();
        db.execute(
            "DELETE FROM connectors WHERE account=?1 AND provider=?2",
            params![c.workspace, provider],
        )?;
        db.execute("UPDATE consents SET withdrawn_at=?1 WHERE workspace=?2 AND kind IN (?3,?4) AND withdrawn_at IS NULL",params![now(),c.workspace,format!("{provider}_email_processing"),format!("{provider}_oauth_grant")])?;
    }
    app.store.event(
        &c.workspace,
        "disconnected",
        "An email connection was removed. Existing invoices remain in the archive.",
    )?;
    Ok(Json(json!({"ok":true})))
}
async fn sync(State(app): State<App>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    let providers: Vec<String> = {
        let db = app.store.db.lock().unwrap();
        let mut stmt = db.prepare("SELECT provider FROM connectors WHERE account=?1")?;
        stmt.query_map([&c.workspace], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?
    };
    if providers.is_empty() {
        return Err(bad("Connect an email account first."));
    }
    for provider in providers {
        let cloned = app.clone();
        let workspace = c.workspace.clone();
        tokio::spawn(async move {
            let _ = mail::sync(&cloned, &workspace, &provider).await;
        });
    }
    Ok(Json(json!({"ok":true})))
}
async fn activity(State(app): State<App>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let c = auth(&app, &headers)?;
    let db = app.store.db.lock().unwrap();
    let mut stmt = db.prepare(
        "SELECT time,kind,message FROM events WHERE account=?1 ORDER BY id DESC LIMIT 30",
    )?;
    let rows=stmt.query_map([&c.workspace],|r|Ok(json!({"time":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"message":r.get::<_,String>(2)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
    Ok(Json(json!({"events":rows})))
}
#[derive(Deserialize)]
struct InviteInput {
    email: String,
}
async fn members(State(app): State<App>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let c = auth(&app, &headers)?;
    let db = app.store.db.lock().unwrap();
    let mut statement = db.prepare("SELECT a.id,a.email,m.role FROM memberships m JOIN accounts a ON a.id=m.user WHERE m.workspace=?1 ORDER BY m.role DESC,a.email")?;
    let rows = statement.query_map([&c.workspace], |r| Ok(json!({"id":r.get::<_,String>(0)?,"email":r.get::<_,String>(1)?,"role":r.get::<_,String>(2)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
    Ok(Json(json!({"members":rows})))
}
async fn remove_member(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    owner(&c)?;
    {
        let mut db = app.store.db.lock().unwrap();
        let tx = db.transaction()?;
        let removed = tx.execute(
            "DELETE FROM memberships WHERE workspace=?1 AND user=?2 AND role='member'",
            params![c.workspace, id],
        )?;
        if removed == 0 {
            return Err(missing());
        }
        tx.execute(
            "DELETE FROM sessions WHERE workspace=?1 AND account=?2",
            params![c.workspace, id],
        )?;
        tx.commit()?;
    }
    app.store.event(
        &c.workspace,
        "access_removed",
        "A member's workspace access was removed.",
    )?;
    Ok(Json(json!({"ok":true})))
}
async fn invite(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<InviteInput>,
) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    owner(&c)?;
    if !body.email.contains('@') || body.email.len() > 254 {
        return Err(bad("Enter the teammate's email address."));
    }
    let token = storage::random_token();
    app.store.db.lock().unwrap().execute(
        "INSERT INTO invitations VALUES (?1,?2,?3,?4,?5)",
        params![
            hash(token.as_bytes()),
            c.workspace,
            c.user,
            body.email.trim().to_lowercase(),
            chrono::Utc::now().timestamp() + 604800
        ],
    )?;
    Ok(Json(
        json!({"url":format!("{}/?invite={token}",app.config.origin)}),
    ))
}
#[derive(Deserialize)]
struct InviteAccept {
    token: String,
}
async fn accept_invite(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<InviteAccept>,
) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    // An invitation is addressed to an email, so only an account that has proven it controls
    // that address may use it. Without outbound mail nobody can verify, so the gate applies
    // only when mail is configured (the operator must configure it for any public deployment).
    if app.mailer.enabled() && !app.store.email_verified(&c.user)? {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Confirm your email address first. We sent you a link when you created the account."
                .into(),
        ));
    }
    let email = app.store.account(&c.user)?["email"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let mut db = app.store.db.lock().unwrap();
    let tx = db.transaction()?;
    let workspace: Option<String> = tx
        .query_row(
            "SELECT workspace FROM invitations WHERE hash=?1 AND email=?2 AND expires>?3",
            params![
                hash(body.token.as_bytes()),
                email,
                chrono::Utc::now().timestamp()
            ],
            |r| r.get(0),
        )
        .optional()?;
    let workspace = workspace
        .ok_or_else(|| bad("The invitation is expired or belongs to another email address."))?;
    tx.execute(
        "INSERT OR IGNORE INTO memberships VALUES (?1,?2,'member')",
        params![workspace, c.user],
    )?;
    tx.execute(
        "DELETE FROM invitations WHERE hash=?1",
        [hash(body.token.as_bytes())],
    )?;
    tx.execute(
        "UPDATE sessions SET workspace=?1 WHERE hash=?2",
        params![workspace, hash(c.session.as_bytes())],
    )?;
    tx.commit()?;
    Ok(Json(json!({"ok":true})))
}
async fn switch_workspace(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let c = mutation(&app, &headers)?;
    let db = app.store.db.lock().unwrap();
    let belongs: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM memberships WHERE user=?1 AND workspace=?2)",
        params![c.user, id],
        |r| r.get(0),
    )?;
    if !belongs {
        return Err(missing());
    }
    db.execute(
        "UPDATE sessions SET workspace=?1 WHERE hash=?2",
        params![id, hash(c.session.as_bytes())],
    )?;
    Ok(Json(json!({"ok":true})))
}
async fn consent_history(State(app): State<App>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let c = auth(&app, &headers)?;
    let db = app.store.db.lock().unwrap();
    let mut stmt=db.prepare("SELECT kind,version,granted_at,withdrawn_at FROM consents WHERE user=?1 AND workspace=?2 ORDER BY id DESC")?;
    let rows=stmt.query_map(params![c.user,c.workspace],|r|Ok(json!({"kind":r.get::<_,String>(0)?,"version":r.get::<_,String>(1)?,"granted_at":r.get::<_,String>(2)?,"withdrawn_at":r.get::<_,Option<String>>(3)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
    Ok(Json(
        json!({"consents":rows,"policy_version":privacy::POLICY_VERSION}),
    ))
}

fn router(app: App) -> Router {
    Router::new()
        .route("/",get(||async {Html(branding::index())}))
        .route("/app.js",get(||async {([(header::CONTENT_TYPE,"text/javascript")],include_str!("../web/app.js"))}))
        .route("/style.css",get(||async {([(header::CONTENT_TYPE,"text/css")],include_str!("../web/style.css"))}))
        .route("/tokens.css",get(||async {([(header::CONTENT_TYPE,"text/css")],include_str!("../web/tokens.css"))}))
        .route("/favicon.svg",get(||async {([(header::CONTENT_TYPE,"image/svg+xml")],include_str!("../web/favicon.svg"))}))
        .route("/fonts/archivo.woff2",get(||async {([(header::CONTENT_TYPE,"font/woff2")],include_bytes!("../web/fonts/archivo.woff2").as_slice())}))
        .route("/fonts/atkinson-next.woff2",get(||async {([(header::CONTENT_TYPE,"font/woff2")],include_bytes!("../web/fonts/atkinson-next.woff2").as_slice())}))
        .route("/fonts/atkinson-mono.woff2",get(||async {([(header::CONTENT_TYPE,"font/woff2")],include_bytes!("../web/fonts/atkinson-mono.woff2").as_slice())}))
        .route("/api/privacy",get(|State(app):State<App>|async move {Json(json!({"version":privacy::POLICY_VERSION,"notice":privacy::NOTICE.replace("{{PRODUCT_NAME}}",branding::NAME),"demo":app.config.demo}))}))
        .route("/api/auth/register",post(register)).route("/api/auth/login",post(login)).route("/api/auth/demo",post(demo)).route("/api/auth/logout",post(logout))
        .route("/healthz",get(healthz))
        .route("/api/auth/options",get(accounts::options)).route("/api/auth/verify",post(accounts::verify)).route("/api/auth/resend-verification",post(accounts::resend_verification)).route("/api/auth/forgot",post(accounts::forgot)).route("/api/auth/reset",post(accounts::reset))
        .route("/api/me",get(me)).route("/api/invoices",get(records)).route("/api/upload",post(upload))
        .route("/api/invoices/{id}",get(invoice)).route("/api/invoices/reread",post(reread_review)).route("/api/invoices/{id}/approve",post(approve)).route("/api/invoices/{id}/exclude",post(exclude)).route("/api/invoices/{id}/delete",post(delete_invoice)).route("/api/invoices/{id}/document",get(document))
        .route("/api/export",get(export)).route("/api/stores",post(add_store)).route("/api/stores/{id}",post(update_store))
        .route("/api/connections",get(connections)).route("/api/connections/{provider}/start",post(connect)).route("/api/connections/{provider}/disconnect",post(disconnect)).route("/api/sync",post(sync))
        .route("/api/oauth/{provider}/callback",get(callback)).route("/api/activity",get(activity))
        .route("/api/invitations",post(invite)).route("/api/invitations/accept",post(accept_invite)).route("/api/workspaces/{id}/select",post(switch_workspace)).route("/api/consents",get(consent_history))
        .route("/api/members",get(members)).route("/api/members/{id}/remove",post(remove_member))
        .layer(DefaultBodyLimit::max(25*1024*1024))
        .layer(middleware::from_fn_with_state(app.clone(),security)).with_state(app)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--extract") {
        let output = worker::extract_child(std::path::Path::new(
            args.get(2).ok_or_else(|| anyhow!("PDF path required"))?,
        ))?;
        println!("{}", serde_json::to_string(&output)?);
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("--healthcheck") {
        // For container HEALTHCHECK: the image has no curl. Exit 0 only for an HTTP 200.
        use std::io::{Read, Write};
        let bind = std::env::var("INVOICE_BIND").unwrap_or("127.0.0.1:3030".into());
        let port = bind.rsplit(':').next().unwrap_or("3030");
        let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{port}"))?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        stream.write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply)?;
        if reply.starts_with("HTTP/1.1 200") || reply.starts_with("HTTP/1.0 200") {
            return Ok(());
        }
        return Err(anyhow!("Health check failed"));
    }
    if args.get(1).map(String::as_str) == Some("--inspect") {
        let output = worker::extract_child(std::path::Path::new(
            args.get(2).ok_or_else(|| anyhow!("PDF path required"))?,
        ))?;
        // Optional extra arguments are the customer's own names or addresses. Set
        // INVOICE_FORCE_GENERIC=1 to bypass the hard-coded supplier layouts.
        let context = extraction::Context {
            force_generic: std::env::var("INVOICE_FORCE_GENERIC").as_deref() == Ok("1"),
            known_suppliers: Default::default(),
            customer: args[3..]
                .iter()
                .map(|v| {
                    v.to_lowercase()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect(),
        };
        let invoice = extraction::parse_invoice_with(
            output["text"].as_str().unwrap_or(""),
            output["pages"].as_u64().unwrap_or(1) as usize,
            &context,
        );
        println!("{}", serde_json::to_string(&invoice)?);
        return Ok(());
    }
    if let Some(path) = std::env::var_os("INVOICE_CONFIG_FILE") {
        dotenvy::from_path(path)?;
    } else {
        dotenvy::dotenv().ok();
    }
    if let Some(command) = args.get(1).map(String::as_str)
        && (command == "--backup" || command == "--verify-backup")
    {
        let data = PathBuf::from(std::env::var("INVOICE_DATA_DIR").unwrap_or("data".into()));
        let target = args
            .get(2)
            .ok_or_else(|| anyhow!("A destination directory (or snapshot file) is required"))?;
        if command == "--backup" {
            let snapshot = backup::create(&data, std::path::Path::new(target))?;
            println!("{}", snapshot.display());
        } else {
            let key = std::env::var_os("INVOICE_ENCRYPTION_KEY_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| data.join("encryption.key"));
            let report = backup::verify(std::path::Path::new(target), &key)?;
            println!(
                "{}",
                json!({"integrity":"ok","accounts":report.accounts,"workspaces":report.workspaces,"invoices":report.invoices,"document_decrypted":report.document_decrypted})
            );
            if report.document_decrypted == Some(false) {
                return Err(anyhow!(
                    "A stored document could not be decrypted with this key"
                ));
            }
        }
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("--test-email") {
        // Sends one plain message with the configured mail backend, to prove the key, the
        // verified sending domain and the sender address work before anyone depends on them.
        let to = args
            .get(2)
            .ok_or_else(|| anyhow!("Usage: --test-email <your address>"))?;
        let mailer = outbound::Mailer::from_env()?;
        if !mailer.enabled() {
            return Err(anyhow!(
                "No mail backend is configured (RESEND_API_KEY or SMTP_URL, and MAIL_FROM)"
            ));
        }
        mailer
            .send(
                to,
                &format!("{} test message", branding::NAME),
                format!(
                    "This message confirms that {} can send email from this deployment.\n",
                    branding::NAME
                ),
            )
            .await?;
        println!(
            "Sent a test message to {to} using {}.",
            mailer.provider().unwrap_or("mail")
        );
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("--check-config") {
        let origin = std::env::var("INVOICE_ORIGIN").unwrap_or("http://localhost:3030".into());
        println!(
            "{}",
            json!({"origin":origin,"google_callback":format!("{origin}/api/oauth/google/callback"),"google_configured":(["GOOGLE_CLIENT_ID","GOOGLE_CLIENT_SECRET"].iter().all(|key|std::env::var(key).is_ok_and(|v|!v.trim().is_empty()))),"microsoft_configured":(["MICROSOFT_CLIENT_ID","MICROSOFT_CLIENT_SECRET"].iter().all(|key|std::env::var(key).is_ok_and(|v|!v.trim().is_empty()))),"mail_provider":outbound::Mailer::from_env().ok().and_then(|m|m.provider()),"sample_login_enabled":std::env::var("INVOICE_DEMO").as_deref()==Ok("1"),"initial_lookback_days":mail::lookback_days()})
        );
        return Ok(());
    }
    let bind = std::env::var("INVOICE_BIND").unwrap_or("127.0.0.1:3030".into());
    let origin = std::env::var("INVOICE_ORIGIN")
        .unwrap_or("http://localhost:3030".into())
        .trim_end_matches('/')
        .to_string();
    let url = url::Url::parse(&origin)?;
    let local = ["localhost", "127.0.0.1", "[::1]"].contains(&url.host_str().unwrap_or(""));
    let demo = std::env::var("INVOICE_DEMO").as_deref() == Ok("1");
    if !local && !origin.starts_with("https://") {
        return Err(anyhow!("Public deployments require an HTTPS origin"));
    }
    if demo && (!local || !bind.starts_with("127.0.0.1:")) {
        return Err(anyhow!("Sample workspace mode is only allowed on loopback"));
    }
    let mailer = outbound::Mailer::from_env()?;
    if !local
        && !mailer.enabled()
        && std::env::var("INVOICE_ALLOW_UNVERIFIED_EMAIL").as_deref() != Ok("1")
    {
        return Err(anyhow!(
            "Public deployments need RESEND_API_KEY (or SMTP_URL) and MAIL_FROM so account emails can be verified and passwords reset"
        ));
    }
    let trusted_proxies = std::env::var("INVOICE_TRUSTED_PROXY")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| {
            v.parse::<std::net::IpAddr>()
                .map_err(|_| anyhow!("INVOICE_TRUSTED_PROXY must list IP addresses"))
        })
        .collect::<Result<Vec<_>>>()?;
    let config = Config {
        origin: origin.clone(),
        demo,
        samples: std::env::var("INVOICE_SAMPLE_DIR").ok().map(PathBuf::from),
        trusted_proxies,
        gmail_api: mail::GMAIL_API.into(),
        graph_api: mail::GRAPH_API.into(),
    };
    let data = PathBuf::from(std::env::var("INVOICE_DATA_DIR").unwrap_or("data".into()));
    let store = Store::open(&data)?;
    store.backfill_store_metadata()?;
    store.backfill_supplier_profiles()?;
    let queue = Arc::new(tokio::sync::Notify::new());
    let app = App {
        store,
        config: Arc::new(config),
        queue,
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .build()?,
        sync_locks: Arc::new(Mutex::new(HashSet::new())),
        auth_attempts: Arc::new(Mutex::new(HashMap::new())),
        mailer,
    };
    let workers = std::env::var("INVOICE_WORKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2)
        .clamp(1, 8);
    for _ in 0..workers {
        tokio::spawn(worker::run(app.clone()));
    }
    tokio::spawn(mail::poll(app.clone()));
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    println!("{} running at {origin}", branding::NAME);
    axum::serve(
        listener,
        router(app).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::client_key;
    use crate::testkit::*;
    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use std::net::IpAddr;

    /// A record waiting for review in the session's workspace, plus the store to assign it to.
    async fn waiting_record(f: &Fixture, session: &Session) -> (String, String) {
        let profile = me(&f.app, session).await;
        let workspace = profile["workspace"].as_str().unwrap().to_string();
        let store = profile["stores"][0]["id"].as_str().unwrap().to_string();
        let id = f
            .app
            .store
            .insert_document(&workspace, "a.pdf", b"%PDF-review", "Upload", None, None)
            .unwrap()
            .unwrap();
        let mut record = f.app.store.record(&workspace, &id).unwrap().unwrap();
        record.status = "review".into();
        f.app
            .store
            .commit_record(&workspace, &record, None)
            .unwrap();
        (id, store)
    }
    #[tokio::test]
    async fn approving_an_invoice_remembers_the_confirmed_supplier_for_that_workspace_only() {
        let f = fixture(false);
        let session = register(&f.app, "owner@example.test").await;
        let other = register(&f.app, "other@example.test").await;
        let (id, store) = waiting_record(&f, &session).await;
        let workspace = me(&f.app, &session).await["workspace"]
            .as_str()
            .unwrap()
            .to_string();
        // Pretend the reader had found this tax number beside the supplier.
        let mut record = f.app.store.record(&workspace, &id).unwrap().unwrap();
        record.invoice.identity_keys = vec!["tax:817712326RT0001".into(), "name:actmeparts".into()];
        let record = f
            .app
            .store
            .commit_record(&workspace, &record, None)
            .unwrap();

        let (status, reply, _) = call(
            &f.app,
            "POST",
            &format!("/api/invoices/{id}/approve"),
            Some(form(&store, record.revision)),
            Some(&session),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        let known = f.app.store.known_suppliers(&workspace).unwrap();
        assert_eq!(
            known.get("tax:817712326RT0001").map(String::as_str),
            Some("Acme Parts Ltd")
        );
        assert_eq!(
            known.get("name:acmepartsltd").map(String::as_str),
            Some("Acme Parts Ltd")
        );

        let other_workspace = me(&f.app, &other).await["workspace"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            f.app
                .store
                .known_suppliers(&other_workspace)
                .unwrap()
                .is_empty()
        );
    }

    fn form(store: &str, revision: i64) -> Value {
        json!({"revision":revision,"supplier":"Acme Parts Ltd","invoice_number":"A-1","invoice_date":"2026-10-01","due_date":"2026-10-31","currency":"CAD","subtotal":"100.00","tax":"5.00","total":"105.00","balance":"","po_number":"","store_id":store})
    }

    #[tokio::test]
    async fn the_review_form_only_accepts_the_offered_currencies() {
        let f = fixture(false);
        let session = register(&f.app, "owner@example.test").await;
        assert_eq!(
            me(&f.app, &session).await["currencies"],
            json!(["CAD", "USD"]),
            "the form reads its list from the server"
        );
        let (id, store) = waiting_record(&f, &session).await;
        let path = format!("/api/invoices/{id}/approve");
        for bad in ["EUR", "", "C$", "CADX"] {
            let mut body = form(&store, 1);
            body["currency"] = json!(bad);
            let (status, reply, _) = call(&f.app, "POST", &path, Some(body), Some(&session)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}");
            assert_eq!(reply["error"], "Choose CAD or USD.");
        }
        let mut body = form(&store, 1);
        body["currency"] = json!("usd");
        let (status, reply, _) = call(&f.app, "POST", &path, Some(body), Some(&session)).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!(reply["invoice"]["invoice"]["currency"], "USD");
        assert_eq!(reply["invoice"]["status"], "approved");
    }

    #[tokio::test]
    async fn the_po_number_is_trimmed_bounded_and_optional() {
        let f = fixture(false);
        let session = register(&f.app, "owner@example.test").await;
        let (id, store) = waiting_record(&f, &session).await;
        let path = format!("/api/invoices/{id}/approve");
        let mut body = form(&store, 1);
        body["po_number"] = json!("x".repeat(101));
        let (status, _, _) = call(&f.app, "POST", &path, Some(body), Some(&session)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let mut body = form(&store, 1);
        body["po_number"] = json!("   ");
        let (status, reply, _) = call(&f.app, "POST", &path, Some(body), Some(&session)).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert!(reply["invoice"]["invoice"]["po_number"].is_null());
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn forwarded_address_is_only_believed_from_a_configured_proxy() {
        let proxies = [ip("127.0.0.1")];
        // The proxy appends the real client last; earlier entries are client-supplied.
        assert_eq!(
            client_key(
                &proxies,
                Some(ip("127.0.0.1")),
                Some("6.6.6.6, 203.0.113.9")
            ),
            "203.0.113.9"
        );
        // A direct caller cannot choose its own rate-limit bucket.
        assert_eq!(
            client_key(&proxies, Some(ip("198.51.100.4")), Some("203.0.113.9")),
            "198.51.100.4"
        );
        // Nothing configured: always the connecting address.
        assert_eq!(
            client_key(&[], Some(ip("127.0.0.1")), Some("203.0.113.9")),
            "127.0.0.1"
        );
        // A garbage header from the proxy falls back to the proxy address.
        assert_eq!(
            client_key(&proxies, Some(ip("127.0.0.1")), Some("not-an-ip")),
            "127.0.0.1"
        );
    }
}
