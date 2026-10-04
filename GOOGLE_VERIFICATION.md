# Google OAuth: what to fill in and what to expect

SumLine reads Gmail with the `gmail.readonly` permission, which Google classes as a **restricted** scope. This page collects the answers Google asks for. It is a working draft, not a promise of approval.

## Where things stand

- **Testing mode** (what you use now): only users you list as test users can connect, up to Google's cap on test users, and a connection expires after about 7 days and must be reconnected. No review is needed. This is the right mode for a private pilot.
- **In production, unverified**: anyone with the link can start the flow but sees an "unverified app" warning, and the number of users is capped until verification.
- **Verified**: removes the warning and cap. For a restricted scope, Google's [API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy) requires passing a security assessment and obtaining a Letter of Assessment from a Google-designated third party, repeated every year. That is slow and can cost money. Decide whether SumLine needs it, or whether per-workspace email forwarding (no Google access at all) is the better product.

## What the consent screen needs

| Field | Value |
|---|---|
| App name | SumLine |
| App home page | `https://sumline.tyred.ca` |
| Privacy policy | `https://sumline.tyred.ca/privacy` (public, no sign-in; contains the Limited Use statement) |
| Terms of service | `https://sumline.tyred.ca/terms` |
| Authorized domain | `tyred.ca` (verify it in Google Search Console) |
| Redirect URI | `https://sumline.tyred.ca/api/oauth/google/callback` |
| Scope | `https://www.googleapis.com/auth/gmail.readonly` |
| Developer contact | the same address as `INVOICE_PRIVACY_EMAIL` |

## Why this scope (draft answer for the review form)

> SumLine helps small retail stores capture supplier invoices. The user connects a mailbox that receives supplier invoices. SumLine searches it only for messages that have PDF attachments (`has:attachment filename:pdf`), downloads those PDFs, reads the invoice details, and shows them to the user for review before they are added to a spreadsheet. It stores the sender, subject and received time of those messages and does not store email body text. A narrower scope cannot do this: `gmail.metadata` does not allow reading attachments, and `gmail.readonly` is the narrowest scope that does. SumLine never sends, modifies or deletes mail, does not use the data for advertising, does not sell or transfer it except to the hosting and email providers named in the privacy policy, and does not use it to train AI or machine-learning models.

## Demo video (what Google expects to see)

1. The consent screen with the app name, and the browser address bar showing the real domain.
2. Creating an account and adding a store.
3. Connecting Gmail: the Google permission screen, showing the requested access.
4. An invoice email arriving, the PDF being imported and appearing under Needs review, and the user approving it.
5. Disconnecting the mailbox and deleting the account (Privacy and consent), showing the data is removed.

## Before submitting

- The privacy policy page must be live on the verified domain and must match what the product does (it is generated from `src/legal.rs`; change the text and bump the version if behaviour changes).
- Every setting in `INVOICE_OPERATOR_*` must be real, because the policy names them.
- Test with a Gmail account that is not yours, using the exact production domain.
