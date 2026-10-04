//! Outbound account email (verification and password reset only).
//!
//! This is separate from `mail.rs`, which reads a customer's invoice mailbox. Nothing here reads
//! or sends invoice content, and message bodies are never written to logs.

use anyhow::{Result, anyhow};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
};

#[derive(Clone)]
pub enum Mailer {
    Disabled,
    Smtp {
        transport: Box<AsyncSmtpTransport<Tokio1Executor>>,
        from: Mailbox,
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
    /// Reads `SMTP_URL` and `MAIL_FROM`. Returns `Disabled` when neither is set.
    pub fn from_env() -> Result<Self> {
        let url = std::env::var("SMTP_URL")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let from = std::env::var("MAIL_FROM")
            .ok()
            .filter(|v| !v.trim().is_empty());
        match (url, from) {
            (None, None) => Ok(Self::Disabled),
            (Some(_), None) | (None, Some(_)) => {
                Err(anyhow!("SMTP_URL and MAIL_FROM must be set together"))
            }
            (Some(url), Some(from)) => {
                let url = validate_smtp_url(url.trim())?;
                let from: Mailbox = from
                    .trim()
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
}
