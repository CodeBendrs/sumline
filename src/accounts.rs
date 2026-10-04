//! Email verification and password reset.
//!
//! Tokens are random, single-use, expire, and only their hash is stored. Responses never reveal
//! whether an address has an account. Neither the tokens nor message bodies are logged.

use crate::{ApiError, ApiResult, App, bad, branding, mutation};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};

const VERIFY_TTL_SECONDS: i64 = 24 * 3600;
const RESET_TTL_SECONDS: i64 = 3600;

pub async fn options(State(app): State<App>) -> Json<Value> {
    Json(json!({
        "mail_enabled": app.mailer.enabled(),
        "signup_code_required": app.config.signup_code.is_some(),
    }))
}

fn valid_email(email: &str) -> bool {
    email.contains('@') && email.len() <= 254
}

fn token_shape_ok(token: &str) -> bool {
    !token.is_empty() && token.len() <= 128
}

fn verification_body(app: &App, token: &str) -> String {
    format!(
        "Confirm this email address for your {name} account by opening this link within 24 hours:\n\n{origin}/?verify={token}\n\nIf you did not create this account, you can ignore this message.\n",
        name = branding::NAME,
        origin = app.config.origin,
    )
}

/// Emails a verification link. Returns whether a message was handed to the mail server.
/// Skips quietly when mail is not configured or one was sent within the cooldown.
pub async fn send_verification(app: &App, account: &str, email: &str) -> bool {
    if !app.mailer.enabled() {
        return false;
    }
    let token = match app
        .store
        .issue_email_token(account, "verify", email, VERIFY_TTL_SECONDS)
    {
        Ok(Some(token)) => token,
        _ => return false,
    };
    let sent = app
        .mailer
        .send(
            email,
            &format!("Confirm your {} email address", branding::NAME),
            verification_body(app, &token),
        )
        .await
        .is_ok();
    if !sent {
        eprintln!("A verification email could not be sent.");
    }
    sent
}

async fn send_reset(app: &App, account: &str, email: &str) {
    let token = match app
        .store
        .issue_email_token(account, "reset", email, RESET_TTL_SECONDS)
    {
        Ok(Some(token)) => token,
        _ => return,
    };
    let body = format!(
        "Someone asked to reset the password for your {name} account. To choose a new password, open this link within one hour:\n\n{origin}/?reset={token}\n\nIf you did not ask for this, ignore this message; your password has not changed.\n",
        name = branding::NAME,
        origin = app.config.origin,
    );
    if app
        .mailer
        .send(
            email,
            &format!("Reset your {} password", branding::NAME),
            body,
        )
        .await
        .is_err()
    {
        eprintln!("A password reset email could not be sent.");
    }
}

#[derive(Deserialize)]
pub struct TokenBody {
    token: String,
}

pub async fn verify(State(app): State<App>, Json(body): Json<TokenBody>) -> ApiResult<Json<Value>> {
    let invalid =
        || bad("This link is invalid or has expired. Request a new one after signing in.");
    if !token_shape_ok(&body.token) {
        return Err(invalid());
    }
    let (account, email) = app
        .store
        .consume_email_token("verify", &body.token)?
        .ok_or_else(invalid)?;
    if !app.store.mark_email_verified(&account, &email)? {
        return Err(invalid());
    }
    Ok(Json(json!({"ok":true})))
}

pub async fn resend_verification(
    State(app): State<App>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let context = mutation(&app, &headers)?;
    if !app.mailer.enabled() {
        return Err(bad("Email is not set up for this deployment."));
    }
    if app.store.email_verified(&context.user)? {
        return Ok(Json(json!({"already_verified":true})));
    }
    let email = app.store.account(&context.user)?["email"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let token = app
        .store
        .issue_email_token(&context.user, "verify", &email, VERIFY_TTL_SECONDS)?
        .ok_or_else(|| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "A message was sent a moment ago. Check your inbox, or wait a minute and try again."
                    .into(),
            )
        })?;
    app.mailer
        .send(
            &email,
            &format!("Confirm your {} email address", branding::NAME),
            verification_body(&app, &token),
        )
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::BAD_GATEWAY,
                "The email could not be sent. Try again later.".into(),
            )
        })?;
    Ok(Json(json!({"ok":true})))
}

#[derive(Deserialize)]
pub struct ForgotBody {
    email: String,
}

/// Always answers the same way. The lookup and the send run after the reply so response time
/// does not depend on whether the address has an account.
pub async fn forgot(
    State(app): State<App>,
    Json(body): Json<ForgotBody>,
) -> ApiResult<Json<Value>> {
    if !app.mailer.enabled() {
        return Err(bad(
            "Password reset by email is not set up for this deployment.",
        ));
    }
    if !valid_email(&body.email) {
        return Err(bad("Enter the email address for your account."));
    }
    tokio::spawn(async move {
        if let Ok(Some((account, email))) = app.store.account_by_email(&body.email) {
            send_reset(&app, &account, &email).await;
        }
    });
    Ok(Json(json!({"ok":true})))
}

#[derive(Deserialize)]
pub struct ResetBody {
    token: String,
    password: String,
}

pub async fn reset(State(app): State<App>, Json(body): Json<ResetBody>) -> ApiResult<Json<Value>> {
    // Check the password first so a rejected password does not use up the link.
    if !(12..=128).contains(&body.password.len()) {
        return Err(bad("Use a password of 12 to 128 characters."));
    }
    let invalid = || bad("This link is invalid or has expired. Request a new reset link.");
    if !token_shape_ok(&body.token) {
        return Err(invalid());
    }
    let (account, _) = app
        .store
        .consume_email_token("reset", &body.token)?
        .ok_or_else(invalid)?;
    let store = app.store.clone();
    let password = body.password.clone();
    tokio::task::spawn_blocking(move || store.reset_password(&account, &password))
        .await
        .map_err(|_| bad("The password could not be changed."))??;
    Ok(Json(json!({"ok":true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;

    #[tokio::test]
    async fn verification_link_works_once_and_only_when_unexpired() {
        let f = fixture(true);
        let session = register(&f.app, "  Owner@Example.test ").await;
        let sent = mail_count(&f.outbox, 1).await;
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to, "owner@example.test");
        assert!(sent[0].subject.contains("Confirm"));
        assert_eq!(me(&f.app, &session).await["user"]["email_verified"], false);

        let token = token_in(&sent[0], "verify");
        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/auth/verify",
            Some(json!({"token":"wrong"})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/auth/verify",
            Some(json!({"token":token})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me(&f.app, &session).await["user"]["email_verified"], true);
        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/auth/verify",
            Some(json!({"token":token})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "a link cannot be reused");

        let other = register(&f.app, "second@example.test").await;
        let sent = mail_count(&f.outbox, 2).await;
        let token = token_in(&sent[1], "verify");
        f.app
            .store
            .db
            .lock()
            .unwrap()
            .execute("UPDATE email_tokens SET expires=1", [])
            .unwrap();
        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/auth/verify",
            Some(json!({"token":token})),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "an expired link is refused"
        );
        assert_eq!(me(&f.app, &other).await["user"]["email_verified"], false);
    }

    #[tokio::test]
    async fn tokens_are_stored_hashed_and_resend_is_rate_limited() {
        let f = fixture(true);
        let session = register(&f.app, "owner@example.test").await;
        let sent = mail_count(&f.outbox, 1).await;
        let token = token_in(&sent[0], "verify");
        let stored: String = f
            .app
            .store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT hash FROM email_tokens", [], |r| r.get(0))
            .unwrap();
        assert_ne!(stored, token, "the raw token is never stored");

        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/auth/resend-verification",
            None,
            Some(&session),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::TOO_MANY_REQUESTS,
            "registration just sent one"
        );
        assert_eq!(f.outbox.lock().unwrap().len(), 1);
        f.app
            .store
            .db
            .lock()
            .unwrap()
            .execute("UPDATE email_tokens SET created=created-3600", [])
            .unwrap();
        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/auth/resend-verification",
            None,
            Some(&session),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let sent = f.outbox.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert_ne!(token_in(&sent[1], "verify"), token);
        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/auth/verify",
            Some(json!({"token":token})),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "issuing a new link replaces the old one"
        );
    }

    #[tokio::test]
    async fn forgot_password_never_reveals_accounts_and_reset_revokes_sessions() {
        let f = fixture(true);
        let session = register(&f.app, "owner@example.test").await;
        mail_count(&f.outbox, 1).await;

        let (status, unknown, _) = call(
            &f.app,
            "POST",
            "/api/auth/forgot",
            Some(json!({"email":"nobody@example.test"})),
            None,
        )
        .await;
        let (status_known, known, _) = call(
            &f.app,
            "POST",
            "/api/auth/forgot",
            Some(json!({"email":"OWNER@example.test"})),
            None,
        )
        .await;
        assert_eq!((status, status_known), (StatusCode::OK, StatusCode::OK));
        assert_eq!(
            unknown, known,
            "identical replies for known and unknown addresses"
        );
        let sent = mail_count(&f.outbox, 2).await;
        assert_eq!(sent.len(), 2, "only the real account received mail");
        assert_eq!(sent[1].to, "owner@example.test");
        assert!(sent[1].subject.contains("Reset"));
        let token = token_in(&sent[1], "reset");

        // A second request inside the cooldown sends nothing more.
        call(
            &f.app,
            "POST",
            "/api/auth/forgot",
            Some(json!({"email":"owner@example.test"})),
            None,
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(f.outbox.lock().unwrap().len(), 2);

        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/auth/reset",
            Some(json!({"token":token,"password":"short"})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let new_password = "a different long password";
        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/auth/reset",
            Some(json!({"token":token,"password":new_password})),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a rejected password does not burn the link"
        );
        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/auth/reset",
            Some(json!({"token":token,"password":new_password})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "the link works once");

        let (status, _, _) = call(&f.app, "GET", "/api/me", None, Some(&session)).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "existing sessions are revoked"
        );
        assert_eq!(
            login(&f.app, "owner@example.test", PASSWORD).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (status, fresh) = login(&f.app, "owner@example.test", new_password).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            me(&f.app, &fresh.unwrap()).await["user"]["email_verified"],
            true
        );
    }

    #[tokio::test]
    async fn invitations_require_a_verified_address_when_mail_is_configured() {
        let f = fixture(true);
        let owner = register(&f.app, "owner@example.test").await;
        let (status, invitation, _) = call(
            &f.app,
            "POST",
            "/api/invitations",
            Some(json!({"email":"teammate@example.test"})),
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let url = invitation["url"].as_str().unwrap();
        let invite = url.split("invite=").nth(1).unwrap().to_string();

        let teammate = register(&f.app, "teammate@example.test").await;
        let (status, body, _) = call(
            &f.app,
            "POST",
            "/api/invitations/accept",
            Some(json!({"token":invite})),
            Some(&teammate),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

        let sent = mail_count(&f.outbox, 2).await;
        let message = sent
            .iter()
            .find(|m| m.to == "teammate@example.test")
            .unwrap();
        call(
            &f.app,
            "POST",
            "/api/auth/verify",
            Some(json!({"token":token_in(message, "verify")})),
            None,
        )
        .await;
        let (status, body, _) = call(
            &f.app,
            "POST",
            "/api/invitations/accept",
            Some(json!({"token":invite})),
            Some(&teammate),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    #[tokio::test]
    async fn without_mail_configured_nothing_is_sent_and_preview_behaviour_is_unchanged() {
        let f = fixture(false);
        let owner = register(&f.app, "owner@example.test").await;
        let (_, options, _) = call(&f.app, "GET", "/api/auth/options", None, None).await;
        assert_eq!(options["mail_enabled"], false);
        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/auth/forgot",
            Some(json!({"email":"owner@example.test"})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/auth/resend-verification",
            None,
            Some(&owner),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(f.outbox.lock().unwrap().is_empty());

        let (_, invitation, _) = call(
            &f.app,
            "POST",
            "/api/invitations",
            Some(json!({"email":"teammate@example.test"})),
            Some(&owner),
        )
        .await;
        let invite = invitation["url"]
            .as_str()
            .unwrap()
            .split("invite=")
            .nth(1)
            .unwrap()
            .to_string();
        let teammate = register(&f.app, "teammate@example.test").await;
        let (status, _, _) = call(
            &f.app,
            "POST",
            "/api/invitations/accept",
            Some(json!({"token":invite})),
            Some(&teammate),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
}
