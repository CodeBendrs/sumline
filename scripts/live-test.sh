#!/bin/sh
# Starts a private instance for live mailbox testing, separate from any other local data:
# its own configuration file, database and encryption key, loopback only, no sample login,
# and a first scan limited to the last day. Put OAuth credentials in the configuration file it
# prints; never in chat or Git. Set SUMLINE_LIVE_TEST_DIR to keep the files somewhere else.
set -eu
repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"
test_dir="${SUMLINE_LIVE_TEST_DIR:-$HOME/.sumline-live-test}"
mkdir -p "$test_dir"
chmod 700 "$test_dir"
if [ ! -f "$test_dir/config.env" ]; then
  cp "$repo/.env.example" "$test_dir/config.env"
  chmod 600 "$test_dir/config.env"
fi
export INVOICE_CONFIG_FILE="$test_dir/config.env"
export INVOICE_DATA_DIR="$test_dir/data"
export INVOICE_BIND=127.0.0.1:3030
export INVOICE_ORIGIN=http://localhost:3030
export INVOICE_DEMO=0
export INVOICE_INITIAL_LOOKBACK_DAYS=1
echo "Live test configuration: $INVOICE_CONFIG_FILE"
echo "Add your Google OAuth app credentials there. Do not paste secrets into chat."
echo "Stop any existing preview server first. Open http://localhost:3030."
if [ -x "$repo/target/release/sumline" ]; then
  exec "$repo/target/release/sumline"
fi
exec cargo run --locked --bin sumline
