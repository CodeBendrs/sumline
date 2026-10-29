#!/bin/sh
# Takes a consistent snapshot of the database, proves it can be restored, and keeps the newest N.
#
#   INVOICE_DATA_DIR=/path/to/data scripts/backup.sh /path/to/backups [keep]
#
# The snapshot does not contain the encryption key. Store the key file separately and test a
# restore (--verify-backup) after every key or storage change. Copy snapshots off this machine.
set -eu
destination=${1:?usage: backup.sh <backup-directory> [snapshots-to-keep]}
keep=${2:-14}
binary=${INVOICE_BIN:-./target/release/sumline}

snapshot=$("$binary" --backup "$destination")
"$binary" --verify-backup "$snapshot" >/dev/null

# Remove everything but the newest snapshots.
ls -1t "$destination"/invoice-tracker-*.sqlite | tail -n +"$((keep + 1))" | while read -r old; do
  rm -f -- "$old"
done
echo "Verified snapshot: $snapshot"
