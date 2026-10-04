#!/bin/sh
# Checks deploy/entrypoint.sh with stand-in commands (no container or root needed):
# as root it fixes ownership of the data directory and starts the app through setpriv as the
# `invoice` user with every argument intact; as anyone else it just starts the app.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/data"
log="$work/log"

cat > "$work/bin/chown" <<STUB
#!/bin/sh
echo "chown \$*" >> "$log"
STUB
cat > "$work/bin/setpriv" <<STUB
#!/bin/sh
echo "setpriv \$1 \$2 \$3" >> "$log"
shift 3
exec "\$@"
STUB
cat > "$work/app" <<STUB
#!/bin/sh
echo "app \$*" >> "$log"
STUB
chmod +x "$work/bin/chown" "$work/bin/setpriv" "$work/app"

run_as() { # $1 = uid reported by id
  printf '#!/bin/sh\necho %s\n' "$1" > "$work/bin/id"; chmod +x "$work/bin/id"
  : > "$log"
  PATH="$work/bin:$PATH" INVOICE_DATA_DIR="$work/data" INVOICE_BIN="$work/app" "$here/deploy/entrypoint.sh" --healthcheck "two words"
}

run_as 0
grep -q "^chown -R invoice:invoice $work/data$" "$log" || { echo "FAIL: root did not fix ownership"; cat "$log"; exit 1; }
grep -q "^setpriv --reuid=invoice --regid=invoice --init-groups$" "$log" || { echo "FAIL: privileges not dropped"; cat "$log"; exit 1; }
grep -q '^app --healthcheck two words$' "$log" || { echo "FAIL: arguments not passed intact"; cat "$log"; exit 1; }

run_as 1000
! grep -q "chown\|setpriv" "$log" || { echo "FAIL: non-root should not chown or setpriv"; cat "$log"; exit 1; }
grep -q '^app --healthcheck two words$' "$log" || { echo "FAIL: non-root start"; cat "$log"; exit 1; }
echo "entrypoint: ok"
