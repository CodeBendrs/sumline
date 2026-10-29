#!/bin/sh
# Container start-up. Volumes (Railway, Docker) are mounted owned by root, which an
# unprivileged app cannot write to. Make the data directory writable, then drop to the
# `invoice` user before the app starts, so the app itself never runs as root.
set -eu
data="${INVOICE_DATA_DIR:-/app/data}"
binary="${INVOICE_BIN:-/app/sumline}"
mkdir -p "$data"
if [ "$(id -u)" = "0" ]; then
  chown -R invoice:invoice "$data"
  exec setpriv --reuid=invoice --regid=invoice --init-groups "$binary" "$@"
fi
exec "$binary" "$@"
