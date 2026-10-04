# First Gmail test

The app is prepared for a local test. Live OAuth and mailbox capture still need to be verified with your account. No Gmail account has been connected during development.

## Google setup (once, for the app operator)

1. In [Google Cloud Console](https://console.cloud.google.com/), create or select a dedicated development project and enable the Gmail API.
2. Configure Google Auth Platform branding and audience. For an External app in Testing, add your receiving Gmail address as a test user. A Workspace-only Internal app must use an eligible organization project.
3. Add `https://www.googleapis.com/auth/gmail.readonly` under Data Access. This app needs read access; it does not send or delete emails. The scope covers the mailbox, even though the app filters candidate PDF attachments. Google's [scope documentation](https://developers.google.com/workspace/gmail/api/auth/scopes) classifies this as restricted; customer deployment requires the applicable verification/assessment process.
4. Create an OAuth client of type **Web application**. Register this exact authorized redirect URI:

   `http://localhost:3030/api/oauth/google/callback`

   Use `localhost` consistently, including in the browser. This is a local Rust web server using Google's [web-server authorization flow](https://developers.google.com/identity/protocols/oauth2/web-server).
5. Run `Live-Test.command` from the Gold project folder once. It creates a private configuration template at `~/Library/Application Support/InvoiceTracker-Live-Test/config.env`. Stop that server with Control-C, add your `GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET` to that file locally, then run the launcher again. Keep secrets out of chat and Git.

The launcher keeps the test database and encryption key in that private Application Support directory, disables the shared sample login, binds to loopback, and limits the first scan to the last day. Stop the existing preview before starting it: both use port 3030. Your Mac and the server must remain running for scheduled checks.

Optional configuration check (prints booleans, not credentials):

```sh
INVOICE_CONFIG_FILE="$HOME/Library/Application Support/InvoiceTracker-Live-Test/config.env" ./bin/invoice-tracker --check-config
```

## Connect your mailbox

1. Open http://localhost:3030 and create your own account/workspace. Choose a separate SumLine password; use your Google password only on Google's sign-in page.
2. Add your stores and a specific identifier from each store's invoice, usually its delivery street address. Routing reads the document, not its sender address.
3. Under **Email connections**, review the mailbox-processing disclosure and privacy notice, select its consent checkbox, and choose **Connect Google account**. Complete Google's sign-in and consent yourself.
4. Check that the connected address is correct. Send test invoices from your other email addresses to this existing receiving address, then use **Check inbox now**, or wait for the five-minute check.

An external Google OAuth app in Testing commonly needs reconnection after seven days because its refresh token expires; see [Google's token expiration rules](https://developers.google.com/identity/protocols/oauth2#expiration). Workspace policies can also restrict authorization. Neither condition is a PDF parsing failure.

## Test cases and expected results

Generate fictional PDFs with `python3 tests/make_mail_fixtures.py`. Poppler's `pdfunite` must be installed. Configure two test stores using identifiers `101 Cedar Avenue` and `202 Maple Road`. The generated invoice dates are 2 October 2026, so choose that month/year when checking totals.

| Send/test | Expected |
|---|---|
| One PDF from each of two sender addresses | Both captured; sender does not determine the store |
| `cedar.pdf` and `maple.pdf` attached to one email | Two invoice records routed to the two stores |
| Send the same PDF again, including a renamed copy | No second invoice or amount |
| `two-stores.pdf`, containing two invoice PDFs | Original attachment archived; independent child records and store assignments |
| Unknown supplier/layout or unclear delivery address | Review required; no guessed store or total included in exports |
| Supplier/store/date/currency filters | Matching rows and validated totals; separate totals per currency |
| Export the selected date range | Only matching validated invoices, with original PDF links |
| Stop/restart while a PDF is processing | Persisted job resumes; an expired worker cannot overwrite it |
| Disconnect Gmail, then send another invoice | Future checks stop; previously captured records remain accessible |

If the individual PDFs were already captured, the bundled copies will be deduplicated against them. Use a fresh test workspace/account for the bundle scenario, or deliberately remove only your disposable test records first. Original bundles may contain invoices whose child records are deleted; see the privacy notice's deletion explanation.

Record the receiving time, invoice number, expected store, expected amount, actual status, and any warning for each case. Confirm the source PDF and spreadsheet contents before calling the live test passed. Scanned/image-only PDFs still require manual review; OCR is not implemented.
