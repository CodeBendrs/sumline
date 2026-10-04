#!/bin/zsh
set -eu
APP_DIR="${0:A:h}"
cd "$APP_DIR"
export INVOICE_DATA_DIR="${INVOICE_DATA_DIR:-$HOME/Library/Application Support/InvoiceTracker}"
echo "Open http://localhost:3030 after the server starts. Press Control-C to stop."
if [[ -x "$APP_DIR/bin/invoice-tracker" ]]; then
    exec "$APP_DIR/bin/invoice-tracker"
fi
exec cargo run --locked --bin invoice-tracker
