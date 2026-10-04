//! Outbound account email (verification and password reset only).
//!
//! This is separate from `mail.rs`, which reads a customer's invoice mailbox. Nothing here reads
//! or sends invoice content, and message bodies are never written to logs.
//!
//! Two ways to send: SMTP, or Resend's HTTPS API. The HTTPS route matters on hosts that block
//! outbound SMTP (Railway does below its Pro plan).

use anyhow::{Result, anyhow};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
};

const RESEND_ENDPOINT: &str = "https://api.resend.com/emails";

#[derive(Clone)]
pub enum Mailer {
    Disabled,
    Smtp {
        transport: Box<AsyncSmtpTransport<Tokio1Executor>>,
        from: Mailbox,
    },
    Resend {
        client: reqwest::Client,
        api_key: String,
        from: String,
        /// Always Resend's address outside tests, which point it at a fake.
        endpoint: String,
    },
    #[cfg(test)]
    Capture(std::sync::Arc<std::sync::Mutex<Vec<Sent>>>),
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub struct Sent {
    pub to: String,
    pub subject: String,
    pub body: String,
}

fn is_loopback(host: &str) -> bool {
    ["localhost", "127.0.0.1", "[::1]", "::1"].contains(&host)
}

/// Rejects configurations that would send credentials or tokens over a plaintext connection.
fn validate_smtp_url(raw: &str) -> Result<url::Url> {
    let url = url::Url::parse(raw).map_err(|_| anyhow!("SMTP_URL is not a valid URL"))?;
    let host = url.host_str().unwrap_or("");
    if host.is_empty() {
        return Err(anyhow!("SMTP_URL needs a host name"));
    }
    match url.scheme() {
        "smtps" => {}
        "smtp" => {
            let starttls_required = url
                .query_pairs()
                .any(|(key, value)| key == "tls" && value == "required");
            if !starttls_required && !is_loopback(host) {
                return Err(anyhow!(
                    "SMTP_URL must use smtps:// or smtp://...?tls=required for remote servers"
                ));
            }
        }
        _ => return Err(anyhow!("SMTP_URL must start with smtps:// or smtp://")),
    }
    Ok(url)
}

impl Mailer {
    /// Reads `RESEND_API_KEY` or `SMTP_URL`, with `MAIL_FROM`. Returns `Disabled` when none are
    /// set.
    pub fn from_env() -> Result<Self> {
        let var = |name: &str| std::env::var(name).ok();
        Self::from_settings(
            var("SMTP_URL").as_deref(),
            var("RESEND_API_KEY").as_deref(),
            var("MAIL_FROM").as_deref(),
        )
    }

    pub fn from_settings(
        smtp_url: Option<&str>,
        resend_key: Option<&str>,
        from: Option<&str>,
    ) -> Result<Self> {
        fn clean(v: Option<&str>) -> Option<&str> {
            v.map(str::trim).filter(|v| !v.is_empty())
        }
        match (clean(smtp_url), clean(resend_key), clean(from)) {
            (None, None, None) => Ok(Self::Disabled),
            (Some(_), Some(_), _) => Err(anyhow!("Set only one of SMTP_URL and RESEND_API_KEY")),
            (None, None, Some(_)) => Err(anyhow!("MAIL_FROM needs SMTP_URL or RESEND_API_KEY")),
            (_, _, None) => Err(anyhow!(
                "MAIL_FROM must be set together with SMTP_URL or RESEND_API_KEY"
            )),
            (Some(url), None, Some(from)) => {
                let url = validate_smtp_url(url)?;
                let from: Mailbox = from
                    .parse()
                    .map_err(|_| anyhow!("MAIL_FROM is not a valid mailbox"))?;
                let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url.as_str())
                    .map_err(|_| anyhow!("SMTP_URL could not be used"))?
                    .timeout(Some(std::time::Duration::from_secs(20)))
                    .build();
                Ok(Self::Smtp {
                    transport: Box::new(transport),
                    from,
                })
            }
            (None, Some(key), Some(from)) => {
                if key.len() < 8 || key.chars().any(char::is_whitespace) {
                    return Err(anyhow!("RESEND_API_KEY does not look like an API key"));
                }
                from.parse::<Mailbox>()
                    .map_err(|_| anyhow!("MAIL_FROM is not a valid mailbox"))?;
                // Redirects are never followed, so the key can only ever go to the endpoint.
                let client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(std::time::Duration::from_secs(20))
                    .build()
                    .map_err(|_| anyhow!("The mail client could not be created"))?;
                Ok(Self::Resend {
                    client,
                    api_key: key.to_string(),
                    from: from.to_string(),
                    endpoint: RESEND_ENDPOINT.into(),
                })
            }
        }
    }

    /// Which backend is configured, for status output. Never includes any credential.
    pub fn provider(&self) -> Option<&'static str> {
        match self {
            Self::Disabled => None,
            Self::Smtp { .. } => Some("smtp"),
            Self::Resend { .. } => Some("resend"),
            #[cfg(test)]
            Self::Capture(_) => Some("capture"),
        }
    }

    pub fn enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }

    pub async fn send(&self, to: &str, subject: &str, body: String) -> Result<()> {
        match self {
            Self::Disabled => Err(anyhow!("Outbound email is not configured")),
            Self::Smtp { transport, from } => {
                let to: Mailbox = to.parse().map_err(|_| anyhow!("Invalid recipient"))?;
                let message = Message::builder()
                    .from(from.clone())
                    .to(to)
                    .subject(subject)
                    .header(ContentType::TEXT_PLAIN)
                    .body(body)
                    .map_err(|_| anyhow!("Message could not be built"))?;
                transport
                    .send(message)
                    .await
                    .map(|_| ())
                    .map_err(|_| anyhow!("The mail server rejected or did not accept the message"))
            }
            Self::Resend {
                client,
                api_key,
                from,
                endpoint,
            } => {
                // Parsing first rejects anything that is not a plain address.
                let to: Mailbox = to.parse().map_err(|_| anyhow!("Invalid recipient"))?;
                let response = client
                    .post(endpoint)
                    .bearer_auth(api_key)
                    .json(&serde_json::json!({
                        "from": from,
                        "to": [to.email.to_string()],
                        "subject": subject,
                        "text": body,
                    }))
                    .send()
                    .await
                    .map_err(|_| anyhow!("The mail service could not be reached"))?;
                if response.status().is_success() {
                    Ok(())
                } else {
                    Err(anyhow!(
                        "The mail service did not accept the message (status {})",
                        response.status().as_u16()
                    ))
                }
            }
            #[cfg(test)]
            Self::Capture(outbox) => {
                outbox.lock().unwrap().push(Sent {
                    to: to.to_string(),
                    subject: subject.to_string(),
                    body,
                });
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_plaintext_smtp_is_rejected() {
        assert!(validate_smtp_url("smtp://user:pw@mail.example.com:25").is_err());
        assert!(
            validate_smtp_url("smtp://user:pw@mail.example.com:587?tls=opportunistic").is_err()
        );
        assert!(validate_smtp_url("ftp://mail.example.com").is_err());
    }

    #[test]
    fn encrypted_or_loopback_smtp_is_accepted() {
        assert!(validate_smtp_url("smtps://user:pw@mail.example.com:465").is_ok());
        assert!(validate_smtp_url("smtp://user:pw@mail.example.com:587?tls=required").is_ok());
        assert!(validate_smtp_url("smtp://127.0.0.1:2525").is_ok());
        assert!(validate_smtp_url("smtp://localhost:2525").is_ok());
    }

    #[test]
    fn exactly_one_backend_is_chosen_and_mistakes_are_refused() {
        let disabled = Mailer::from_settings(None, None, None).unwrap();
        assert!(!disabled.enabled());
        let from = Some("SumLine <no-reply@mail.example.test>");
        assert_eq!(
            Mailer::from_settings(None, Some("re_abcdefgh1234"), from)
                .unwrap()
                .provider(),
            Some("resend")
        );
        assert_eq!(
            Mailer::from_settings(Some("smtp://127.0.0.1:2525"), None, from)
                .unwrap()
                .provider(),
            Some("smtp")
        );
        // Both at once is ambiguous; a sender without a backend, or a backend without a
        // sender, is a half-finished setup; keys and addresses must look right.
        assert!(
            Mailer::from_settings(Some("smtp://127.0.0.1:2525"), Some("re_abcdefgh1234"), from)
                .is_err()
        );
        assert!(Mailer::from_settings(None, None, from).is_err());
        assert!(Mailer::from_settings(None, Some("re_abcdefgh1234"), None).is_err());
        assert!(Mailer::from_settings(Some("smtp://127.0.0.1:2525"), None, None).is_err());
        assert!(Mailer::from_settings(None, Some("short"), from).is_err());
        assert!(Mailer::from_settings(None, Some("re_has a space"), from).is_err());
        assert!(
            Mailer::from_settings(None, Some("re_abcdefgh1234"), Some("not an address")).is_err()
        );
    }
}

/// The Resend backend against a fake API that records what it receives.
#[cfg(test)]
mod resend {
    use super::*;
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode, header},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    const KEY: &str = "re_testkey1234567";

    #[derive(Default)]
    struct Fake {
        requests: Mutex<Vec<(HeaderMap, Value)>>,
        status: Mutex<u16>,
        elsewhere_hits: Mutex<u32>,
    }

    async fn emails(
        State(s): State<Arc<Fake>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Response {
        s.requests.lock().unwrap().push((headers, body));
        match *s.status.lock().unwrap() {
            302 => (StatusCode::FOUND, [(header::LOCATION, "/elsewhere")]).into_response(),
            0 | 200 => Json(json!({"id":"49a3999c-0ce1-4ea6-ab68-afcd6dc2e794"})).into_response(),
            code => (
                StatusCode::from_u16(code).unwrap(),
                Json(json!({"name":"validation_error","message":"invalid"})),
            )
                .into_response(),
        }
    }
    async fn elsewhere(State(s): State<Arc<Fake>>) -> StatusCode {
        *s.elsewhere_hits.lock().unwrap() += 1;
        StatusCode::OK
    }

    async fn start() -> (Mailer, Arc<Fake>) {
        let fake: Arc<Fake> = Arc::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/emails", post(emails))
            .route("/elsewhere", get(elsewhere))
            .with_state(fake.clone());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mailer = Mailer::Resend {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            api_key: KEY.into(),
            from: "SumLine <no-reply@mail.example.test>".into(),
            endpoint: format!("{base}/emails"),
        };
        (mailer, fake)
    }

    #[tokio::test]
    async fn a_message_is_posted_as_resend_expects() {
        let (mailer, fake) = start().await;
        mailer
            .send(
                "Owner <owner@example.test>",
                "Reset your SumLine password",
                "the body".into(),
            )
            .await
            .unwrap();
        let requests = fake.requests.lock().unwrap();
        let (headers, body) = &requests[0];
        assert_eq!(headers["authorization"], format!("Bearer {KEY}"));
        assert_eq!(headers["content-type"], "application/json");
        assert_eq!(body["from"], "SumLine <no-reply@mail.example.test>");
        assert_eq!(
            body["to"],
            json!(["owner@example.test"]),
            "the address alone, as a list"
        );
        assert_eq!(body["subject"], "Reset your SumLine password");
        assert_eq!(body["text"], "the body");
        assert!(body.get("html").is_none());
    }

    #[tokio::test]
    async fn refusals_and_outages_are_errors_that_never_contain_the_key() {
        let (mailer, fake) = start().await;
        for code in [401u16, 403, 422, 429] {
            *fake.status.lock().unwrap() = code;
            let error = mailer
                .send("owner@example.test", "s", "b".into())
                .await
                .unwrap_err();
            let text = format!("{error:#}");
            assert!(text.contains(&code.to_string()), "{text}");
            assert!(!text.contains(KEY), "the key must never appear in an error");
        }
        // Nothing listening at all.
        let Mailer::Resend {
            client,
            api_key,
            from,
            ..
        } = mailer
        else {
            unreachable!()
        };
        let down = Mailer::Resend {
            client,
            api_key,
            from,
            endpoint: "http://127.0.0.1:9/emails".into(),
        };
        let text = format!(
            "{:#}",
            down.send("owner@example.test", "s", "b".into())
                .await
                .unwrap_err()
        );
        assert!(!text.contains(KEY));
    }

    #[tokio::test]
    async fn the_key_is_never_sent_anywhere_a_redirect_points() {
        let (mailer, fake) = start().await;
        *fake.status.lock().unwrap() = 302;
        assert!(
            mailer
                .send("owner@example.test", "s", "b".into())
                .await
                .is_err()
        );
        assert_eq!(
            *fake.elsewhere_hits.lock().unwrap(),
            0,
            "the redirect was not followed"
        );
    }

    #[tokio::test]
    async fn a_recipient_that_is_not_a_plain_address_is_refused_before_anything_is_sent() {
        let (mailer, fake) = start().await;
        for bad in ["not an address", "a@b.test\r\nBcc: x@y.test", ""] {
            assert!(mailer.send(bad, "s", "b".into()).await.is_err(), "{bad:?}");
        }
        assert!(fake.requests.lock().unwrap().is_empty());
    }
}
