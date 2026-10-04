//! The privacy policy and terms of service.
//!
//! Both are templates filled in from the operator's own details (`Operator`), so a deployment
//! states who is responsible, where data is kept and how long backups last. They are served as
//! public pages (`/privacy`, `/terms`), which Google's OAuth consent screen needs, and inside the
//! app. They describe what the software actually does; if behaviour changes, change the text and
//! bump `POLICY_VERSION`.

/// Changes whenever either document changes materially. Recorded with each consent.
pub const POLICY_VERSION: &str = "2026-10-04-v1";
pub const EFFECTIVE: &str = "4 October 2026";

/// Who runs this deployment. Everything here must be true of the real service.
#[derive(Clone, Debug, Default)]
pub struct Operator {
    pub name: Option<String>,
    pub address: Option<String>,
    pub privacy_email: Option<String>,
    pub governing_law: Option<String>,
    pub hosting_provider: Option<String>,
    pub hosting_region: Option<String>,
    pub backup_retention: Option<String>,
}

fn setting(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl Operator {
    pub fn from_env() -> Self {
        Self {
            name: setting("INVOICE_OPERATOR_NAME"),
            address: setting("INVOICE_OPERATOR_ADDRESS"),
            privacy_email: setting("INVOICE_PRIVACY_EMAIL"),
            governing_law: setting("INVOICE_GOVERNING_LAW"),
            hosting_provider: setting("INVOICE_HOSTING_PROVIDER"),
            hosting_region: setting("INVOICE_HOSTING_REGION"),
            backup_retention: setting("INVOICE_BACKUP_RETENTION"),
        }
    }

    /// Settings a public deployment must provide before it names itself to customers.
    pub fn missing(&self) -> Vec<&'static str> {
        [
            (&self.name, "INVOICE_OPERATOR_NAME"),
            (&self.address, "INVOICE_OPERATOR_ADDRESS"),
            (&self.privacy_email, "INVOICE_PRIVACY_EMAIL"),
            (&self.governing_law, "INVOICE_GOVERNING_LAW"),
            (&self.hosting_provider, "INVOICE_HOSTING_PROVIDER"),
            (&self.hosting_region, "INVOICE_HOSTING_REGION"),
        ]
        .into_iter()
        .filter(|(value, _)| value.is_none())
        .map(|(_, name)| name)
        .collect()
    }

    fn value(setting: &Option<String>, variable: &str) -> String {
        setting
            .clone()
            .unwrap_or_else(|| format!("[not set: {variable}]"))
    }
}

/// Fills a template. `mail_provider` is what actually sends account email, if anything.
fn fill(template: &str, operator: &Operator, origin: &str, mail_provider: Option<&str>) -> String {
    let email_line = match mail_provider {
        Some("resend") => {
            "- Account email: Resend (resend.com) delivers the verification and password-reset messages we send you. It receives your email address and the message.\n"
        }
        Some(_) => {
            "- Account email: our email delivery provider carries the verification and password-reset messages we send you. It receives your email address and the message.\n"
        }
        None => "",
    };
    template
        .replace("{{EMAIL_PROVIDER_LINE}}\n", email_line)
        .replace("{{PRODUCT_NAME}}", crate::branding::NAME)
        .replace(
            "{{OPERATOR_NAME}}",
            &Operator::value(&operator.name, "INVOICE_OPERATOR_NAME"),
        )
        .replace(
            "{{OPERATOR_ADDRESS}}",
            &Operator::value(&operator.address, "INVOICE_OPERATOR_ADDRESS"),
        )
        .replace(
            "{{PRIVACY_EMAIL}}",
            &Operator::value(&operator.privacy_email, "INVOICE_PRIVACY_EMAIL"),
        )
        .replace(
            "{{GOVERNING_LAW}}",
            &Operator::value(&operator.governing_law, "INVOICE_GOVERNING_LAW"),
        )
        .replace(
            "{{HOSTING_PROVIDER}}",
            &Operator::value(&operator.hosting_provider, "INVOICE_HOSTING_PROVIDER"),
        )
        .replace(
            "{{HOSTING_REGION}}",
            &Operator::value(&operator.hosting_region, "INVOICE_HOSTING_REGION"),
        )
        .replace(
            "{{BACKUP_RETENTION}}",
            operator.backup_retention.as_deref().unwrap_or("90 days"),
        )
        .replace("{{EFFECTIVE}}", EFFECTIVE)
        .replace("{{VERSION}}", POLICY_VERSION)
        .replace("{{SITE}}", origin)
}

pub fn privacy_text(operator: &Operator, origin: &str, mail_provider: Option<&str>) -> String {
    fill(PRIVACY, operator, origin, mail_provider)
}
pub fn terms_text(operator: &Operator, origin: &str) -> String {
    fill(TERMS, operator, origin, None)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Escaped text with email addresses made into mail links.
fn inline(text: &str) -> String {
    let escaped = escape(text);
    let address =
        regex::Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+").unwrap();
    address
        .replace_all(&escaped, r#"<a href="mailto:$0">$0</a>"#)
        .into_owned()
}

/// Turns the document format (# and ## headings, "- " lists, paragraphs) into HTML. Lines are
/// read one at a time, so a heading may be followed directly by its text. Everything is escaped
/// first, so an operator's settings cannot inject markup.
pub fn to_html(text: &str) -> String {
    enum Open {
        None,
        List,
        Paragraph(Vec<String>),
    }
    fn close(html: &mut String, open: &mut Open) {
        match std::mem::replace(open, Open::None) {
            Open::None => {}
            Open::List => html.push_str("</ul>\n"),
            Open::Paragraph(lines) => {
                html.push_str(&format!("<p>{}</p>\n", inline(&lines.join(" "))))
            }
        }
    }
    let mut html = String::new();
    let mut open = Open::None;
    for line in text.lines().map(str::trim_end) {
        if line.is_empty() {
            close(&mut html, &mut open);
        } else if let Some(heading) = line.strip_prefix("## ") {
            close(&mut html, &mut open);
            html.push_str(&format!("<h2>{}</h2>\n", inline(heading)));
        } else if let Some(heading) = line.strip_prefix("# ") {
            close(&mut html, &mut open);
            html.push_str(&format!("<h1>{}</h1>\n", inline(heading)));
        } else if let Some(item) = line.strip_prefix("- ") {
            if !matches!(open, Open::List) {
                close(&mut html, &mut open);
                html.push_str("<ul>\n");
                open = Open::List;
            }
            html.push_str(&format!("<li>{}</li>\n", inline(item)));
        } else {
            match &mut open {
                Open::Paragraph(lines) => lines.push(line.to_string()),
                _ => {
                    close(&mut html, &mut open);
                    open = Open::Paragraph(vec![line.to_string()]);
                }
            }
        }
    }
    close(&mut html, &mut open);
    html
}

/// A complete public page for one document.
pub fn page(title: &str, text: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>{title} · {product}</title>
    <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
    <link rel="stylesheet" href="/tokens.css" />
    <link rel="stylesheet" href="/style.css" />
  </head>
  <body class="legal-page">
    <header class="legal-header"><a href="/">← {product}</a></header>
    <main class="legal-article">
{body}    </main>
    <footer class="legal-footer"><a href="/privacy">Privacy policy</a> · <a href="/terms">Terms of service</a></footer>
  </body>
</html>
"#,
        title = escape(title),
        product = escape(crate::branding::NAME),
        body = to_html(text),
    )
}

const PRIVACY: &str = r##"# {{PRODUCT_NAME}} Privacy Policy
Version {{VERSION}}. Effective {{EFFECTIVE}}.

## Who we are
{{PRODUCT_NAME}} ("we", "us") is operated by {{OPERATOR_NAME}}, {{OPERATOR_ADDRESS}}. We are responsible for the personal information under our control. For questions, requests to access, correct or delete information, or complaints, contact our privacy contact at {{PRIVACY_EMAIL}}.

## What this policy covers
This policy explains what information {{PRODUCT_NAME}} collects when you use {{SITE}}, why we collect it, who can see it, where it is kept, how long we keep it, and the choices you have. {{PRODUCT_NAME}} helps stores capture supplier invoices (PDF files) from uploads or from a mailbox they connect, review them, and keep spreadsheet records.

## Information we collect
- Account information: your email address, a hashed password (we cannot read it), your workspace and store names, your role in each workspace, and when you signed up.
- Invoices: the original PDF and the details we read from it, such as supplier, invoice number, dates, line items, amounts, taxes and the store it belongs to. Invoices can contain names, addresses, phone numbers, email addresses and payment or bank details of your suppliers, their staff and you.
- Mailbox information, only if you connect Gmail or Outlook: the sender, subject and received time of messages that carry PDF attachments, identifiers of messages we have already processed, a position marker that lets us check for new mail efficiently, and the access tokens that let us read the mailbox, which are stored encrypted. We do not store the body text of your emails.
- Supplier memory: when you approve an invoice, we remember the supplier name you confirmed together with identifiers found on the invoice, such as its tax registration number, so later invoices from the same supplier are recognised.
- Your choices and history: when you accepted this policy and our terms and which version, when you connected or disconnected a mailbox, and a short activity log for your workspace (for example, "an invoice needs review"), which does not contain invoice contents.
- Technical information: our hosting provider may log connection details such as your IP address and the time of a request for operation and security. We also keep recent sign-in attempt counts by IP address in memory to limit abuse; they are not saved.

## Why we use it
- To provide the service: read invoices, route them to stores, flag uncertain details for your review, build your spreadsheets and keep your archive.
- To run and secure your account: sign-in, verification and password-reset emails, preventing abuse, and fixing faults.
- To meet legal obligations.

We do not sell personal information and do not use it for advertising. We do not use invoice or mailbox contents to train artificial-intelligence or machine-learning models. Invoices are read by our own software on our own servers; their contents are not sent to third-party AI services.

## Your consent
We collect, use and disclose personal information with your consent. You give it when you create an account, for processing the invoices you provide, and separately if you connect a mailbox. You can withdraw consent at any time by disconnecting the mailbox, deleting invoices or deleting your account; if you do, we may no longer be able to provide the service. You are responsible for having the authority to share the invoices and the mailbox you connect, including the personal information of other people they contain.

## Google user data
If you connect Gmail, {{PRODUCT_NAME}} asks for read-only access (the gmail.readonly permission). We use it only to find messages that have PDF attachments, to download those PDFs and the information described above, and to show them to you in the service.

{{PRODUCT_NAME}}'s use and transfer to any other app of information received from Google APIs will adhere to the Google API Services User Data Policy, including the Limited Use requirements.

We do not use Google user data for advertising, we do not sell it, and we do not use it to train artificial-intelligence or machine-learning models. No person at {{PRODUCT_NAME}} reads your messages unless you ask us to for support, it is necessary for security or to investigate abuse, or the law requires it. Because the permission covers the whole mailbox, we recommend connecting a mailbox used for supplier invoices. You can disconnect it at any time in the service, and you can remove {{PRODUCT_NAME}}'s access in your Google Account under Security, then Third-party access.

If you connect Outlook, we ask for the read-only Mail.Read permission and handle that mailbox in the same way; you can remove access in your Microsoft account.

## Who we share information with
We do not share personal information except with the service providers that help us run {{PRODUCT_NAME}}, and only as far as needed for that purpose:
- Hosting: {{HOSTING_PROVIDER}} runs the servers that hold the application, its database and backups, in {{HOSTING_REGION}}.
{{EMAIL_PROVIDER_LINE}}
- Google or Microsoft, only if you connect a mailbox: they provide the mailbox data you authorize us to read.
- Authorities or other parties, only where the law requires it or to protect people's rights, safety or the security of the service.
- A successor, if the service is transferred to another operator; they would be bound by this policy and we would tell you first.

People you invite to your workspace can see every store and invoice in it. We do not disclose invoice contents to anyone else.

## Where information is kept
Information is stored on servers in {{HOSTING_REGION}}, which may be outside Canada. It is then subject to the laws of that place, and its courts and authorities may be able to access it. Our service providers process information on our behalf and we remain responsible for it.

## How we protect it
- Original PDFs, extracted records, spreadsheets and mailbox access tokens are encrypted by the application (AES-256-GCM) before they are stored, and the encryption key is kept apart from the database.
- Passwords are stored only as salted hashes (Argon2).
- Connections use HTTPS. Session cookies are HttpOnly and Secure, and actions that change data require a request token.
- Access is limited by workspace membership, and each workspace is kept separate from the others.
- PDFs are read in a restricted process with time and size limits. We do not run code or open links contained in invoices.
- We take regular backups so that a failure does not lose your records.

No system is perfectly secure. If a breach of security safeguards creates a real risk of significant harm, we will tell the people affected and the Privacy Commissioner of Canada as the law requires, and we keep a record of every breach.

## How long we keep it
Invoices and their records stay until you delete them or delete your account. Deleted information can remain in encrypted backups for up to {{BACKUP_RETENTION}} until those backups expire, and we restore from a backup only to recover from a failure. Consent records are deleted with your account. Logs kept by our hosting provider follow its own retention.

## Your rights and choices
- Access: you can download original invoices and your spreadsheets in the service, and you can ask us for other personal information we hold about you.
- Correction: you can edit invoice details in the service, and ask us to correct your account information.
- Deleting invoices: delete them one at a time in the service.
- Deleting your account: open Privacy and consent, then Delete account. This permanently deletes every workspace in which you are the only member, with its invoices, PDFs, spreadsheets, stores, mailbox connections and consent records. If you share a workspace you are removed from it, and if you own a workspace that has other members you must remove them first.
- Withdrawing consent: disconnect a mailbox in the service, or delete your account.
- Questions and complaints: write to {{PRIVACY_EMAIL}}. We answer within 30 days. You can also contact the Office of the Privacy Commissioner of Canada (priv.gc.ca) or your provincial privacy authority.

## Cookies and similar technologies
We use one cookie, which keeps you signed in. It is needed for the service to work, lasts 12 hours and is not used for tracking. Your browser's local storage keeps interface preferences such as the width of the sidebar. We do not use advertising, analytics or third-party tracking cookies.

## Email we send
We send only the messages needed to run your account: email-address verification and password-reset links. We do not send marketing email.

## Children
{{PRODUCT_NAME}} is a business tool and is not meant for anyone under the age of majority. We do not knowingly collect information from them.

## Changes to this policy
If we change this policy in a way that matters, we will tell you in the service or by email before it takes effect, and ask for your consent again where the law requires it. The version and date are at the top.

## Contact
{{OPERATOR_NAME}}, {{OPERATOR_ADDRESS}}. Privacy contact: {{PRIVACY_EMAIL}}.
"##;

const TERMS: &str = r##"# {{PRODUCT_NAME}} Terms of Service
Version {{VERSION}}. Effective {{EFFECTIVE}}.

## About these terms
These terms are an agreement between you and {{OPERATOR_NAME}}, {{OPERATOR_ADDRESS}} ("we", "us"), the operator of {{PRODUCT_NAME}} at {{SITE}}. By creating an account or using the service you accept these terms and our Privacy Policy. If you use the service for an organisation, you confirm that you have authority to accept them for it.

## The service
{{PRODUCT_NAME}} helps stores capture supplier invoices from uploaded PDFs or a connected mailbox, route them to stores, flag details that need review, and maintain spreadsheets and an archive. The service is offered as a pilot: features can change, and it may be interrupted or contain faults. We do not promise any level of availability.

## Accounts
You must give accurate information, keep your password confidential, and tell us promptly if you think your account was accessed without permission. You are responsible for what happens under your account and for the people you invite to your workspace, who can see every store and invoice in it. The service is for business use by adults.

## Your content and your authority
"Your content" means the invoices, mailbox contents and other information you upload or connect. You keep all rights in it. You give us permission to store, process and display it only as needed to provide the service to you, as described in our Privacy Policy. You confirm that you have the right and authority to share your content with us, including a mailbox you connect and the personal information of other people it contains, and that doing so does not break the law or anyone's rights.

## Acceptable use
You agree not to:
- use the service unlawfully, or to handle content you have no right to use;
- upload malware or content designed to harm, overload or probe the service;
- try to access another person's account or workspace, or bypass security or usage limits;
- connect a mailbox you are not authorised to connect;
- resell the service or use it to build a competing one, except as we agree in writing.

We may suspend access that we reasonably believe breaks these terms or threatens the service or other users.

## You must check the results
Extraction is automated and can be wrong, including supplier names, numbers, dates, currencies, amounts, taxes and line items. The service flags details it is unsure about for your review, but it cannot catch everything. You are responsible for checking invoices, spreadsheets and totals before you rely on them, including for accounting, payment, tax or reporting. {{PRODUCT_NAME}} does not provide accounting, tax or legal advice, and does not pay or approve any invoice.

## Email accounts and other services
If you connect Gmail or Outlook, you authorise us to read that mailbox as described in our Privacy Policy. Google and Microsoft have their own terms, which apply to your use of their services. You can disconnect a mailbox at any time and remove our access in your Google or Microsoft account.

## Fees
The pilot is free unless we agree otherwise in writing. If we introduce fees we will tell you in advance and you can stop using the service instead of paying.

## Your data and our responsibilities as a processor
You decide what to put into the service. We process your content only to provide the service to you, on your instructions as given through the service. We keep it confidential, protect it as described in our Privacy Policy, use the service providers listed there under terms that require them to protect it, and tell you without undue delay if we learn of a breach of security safeguards that affects your content. You can download your original invoices and spreadsheets at any time. When you delete content or your account, we delete it as described in the Privacy Policy, apart from backups that expire on their own schedule.

## Ownership
We and our licensors own the service, including its software, design and name. These terms give you a right to use it, not ownership of it. If you send us feedback, we may use it to improve the service without owing you anything.

## Ending your use
You can stop using the service and delete your account at any time (Privacy and consent, then Delete account). We may end or restrict your access if you break these terms, if we need to for legal or security reasons, or on reasonable notice if we discontinue the service. After your account ends, your content is deleted as described in the Privacy Policy. Sections that by their nature should continue to apply, including the limits on our liability, continue after that.

## Disclaimers
To the extent the law allows, the service is provided "as is" and "as available", without promises that it will be uninterrupted, error-free or fit for a particular purpose, and without a promise that extraction will be accurate or complete. We take reasonable steps, including regular backups, to protect your content, but we do not guarantee that it will never be lost, so keep your own copies of anything you cannot afford to lose.

## Limits on our liability
To the extent the law allows, we are not liable for indirect or consequential losses, lost profits, lost savings, lost data, or business interruption, or for losses that come from relying on extracted information without checking it. To the extent the law allows, our total liability for anything arising from the service or these terms is limited to the greater of the fees you paid us in the 12 months before the claim and CAD $100. Nothing in these terms limits liability that the law does not allow us to limit.

## Your responsibility for claims
You are responsible for claims by others arising from your content or from your use of the service in breach of these terms, and you will reimburse our reasonable losses from such claims to the extent the law allows.

## Changes to these terms
We may update these terms. If a change matters, we will tell you in the service or by email before it takes effect. If you keep using the service after the change takes effect, you accept the updated terms; if you do not agree, you can stop using the service and delete your account.

## Governing law
These terms are governed by the laws of {{GOVERNING_LAW}} and the federal laws of Canada that apply there. Courts located in {{GOVERNING_LAW}} have exclusive jurisdiction over disputes about them, unless the law gives you the right to bring a claim somewhere else.

## General
These terms and the Privacy Policy are the whole agreement between us about the service. If a part is found unenforceable, the rest still applies. If we do not enforce a right, we have not given it up. You may not transfer your rights under these terms without our written consent; we may transfer ours with the service. We are not responsible for delays or failures caused by events beyond our reasonable control.

## Contact
{{OPERATOR_NAME}}, {{OPERATOR_ADDRESS}}. Questions about these terms or the service: {{PRIVACY_EMAIL}}.
"##;

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> Operator {
        Operator {
            name: Some("Example Retail Inc.".into()),
            address: Some("100 Main Street, Victoria, BC".into()),
            privacy_email: Some("privacy@example.test".into()),
            governing_law: Some("British Columbia".into()),
            hosting_provider: Some("Railway".into()),
            hosting_region: Some("United States (US East)".into()),
            backup_retention: Some("90 days".into()),
        }
    }

    #[test]
    fn every_placeholder_is_filled_for_a_complete_operator() {
        for text in [
            privacy_text(&complete(), "https://sumline.example.test", Some("resend")),
            terms_text(&complete(), "https://sumline.example.test"),
        ] {
            assert!(!text.contains("{{"), "an unfilled placeholder remains");
            assert!(!text.contains("[not set"), "a missing value shows");
            assert!(text.contains("Example Retail Inc."));
            assert!(text.contains(POLICY_VERSION));
        }
    }

    #[test]
    fn unset_details_are_visible_and_listed_so_nobody_launches_with_blanks() {
        let blank = Operator::default();
        assert_eq!(
            blank.missing(),
            [
                "INVOICE_OPERATOR_NAME",
                "INVOICE_OPERATOR_ADDRESS",
                "INVOICE_PRIVACY_EMAIL",
                "INVOICE_GOVERNING_LAW",
                "INVOICE_HOSTING_PROVIDER",
                "INVOICE_HOSTING_REGION"
            ]
        );
        assert!(complete().missing().is_empty());
        let text = privacy_text(&blank, "http://localhost:3030", None);
        assert!(text.contains("[not set: INVOICE_OPERATOR_NAME]"));
        assert!(!text.contains("{{"));
        // The backup period has a stated default; it must match what the operator really does.
        assert!(text.contains("90 days"));
    }

    #[test]
    fn the_privacy_policy_says_what_the_product_does_and_google_requires_it_to_say() {
        let text = privacy_text(&complete(), "https://sumline.example.test", Some("resend"));
        for required in [
            "adhere to the Google API Services User Data Policy, including the Limited Use requirements",
            "gmail.readonly",
            "do not use it to train artificial-intelligence or machine-learning models",
            "do not sell personal information",
            "Privacy Commissioner of Canada",
            "Delete account",
            "We do not store the body text of your emails",
            "Resend (resend.com)",
            "AES-256-GCM",
            "within 30 days",
        ] {
            assert!(text.contains(required), "missing: {required}");
        }
        // No email provider configured: that line is not claimed.
        assert!(!privacy_text(&complete(), "https://x.test", None).contains("Resend"));
    }

    #[test]
    fn the_terms_cover_checking_results_authority_and_how_to_leave() {
        let text = terms_text(&complete(), "https://sumline.example.test");
        for required in [
            "You must check the results",
            "does not provide accounting, tax or legal advice",
            "authority to share your content",
            "Delete account",
            "governed by the laws of British Columbia",
            "Nothing in these terms limits liability that the law does not allow us to limit",
        ] {
            assert!(text.contains(required), "missing: {required}");
        }
    }

    #[test]
    fn pages_escape_everything_and_render_headings_lists_and_mail_links() {
        let mut operator = complete();
        operator.name = Some("<script>alert(1)</script> & Co".into());
        let html = page(
            "Privacy policy",
            &privacy_text(&operator, "https://x.test", None),
        );
        assert!(!html.contains("<script>"), "settings cannot inject markup");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; Co"));
        assert!(html.contains("<h1>"), "title");
        assert!(html.contains("<h2>Who we are</h2>"));
        assert!(html.contains("<ul>") && html.contains("<li>"));
        assert!(html.contains(r#"<a href="mailto:privacy@example.test">privacy@example.test</a>"#));
        assert!(html.contains(r#"href="/terms""#) && html.contains(r#"href="/style.css""#));
        assert!(
            !html.contains("style=\""),
            "no inline styles: the page must satisfy the CSP"
        );
    }
}
