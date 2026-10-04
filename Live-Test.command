#!/bin/zsh
set -eu
APP_DIR="${0:A:h}"
cd "$APP_DIR"
TEST_DIR="$HOME/Library/Application Support/InvoiceTracker-Live-Test"
mkdir -p "$TEST_DIR"
chmod 700 "$TEST_DIR"
if [[ ! -f "$TEST_DIR/config.env" ]]; then
  cp "$APP_DIR/.env.example" "$TEST_DIR/config.env"
  chmod 600 "$TEST_DIR/config.env"
fi
export INVOICE_CONFIG_FILE="$TEST_DIR/config.env"
export INVOICE_DATA_DIR="$TEST_DIR/data"
export INVOICE_BIND=127.0.0.1:3030
export INVOICE_ORIGIN=http://localhost:3030
export INVOICE_DEMO=0
export INVOICE_INITIAL_LOOKBACK_DAYS=1
print "Live test configuration: $INVOICE_CONFIG_FILE"
print "Add your Google OAuth app credentials there. Do not paste secrets into chat."
print "Stop any existing preview server first. Open http://localhost:3030."
if [[ -x "$APP_DIR/bin/invoice-tracker" ]]; then
  exec "$APP_DIR/bin/invoice-tracker"
fi
exec cargo run --locked --bin invoice-tracker
