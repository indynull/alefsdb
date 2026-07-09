#!/usr/bin/env bash
# Soak: many CLI writes then compact; prints WAL size before/after.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$ROOT/target/release/alefsdb}"
if [[ ! -x "$BIN" ]]; then
  BIN="$ROOT/target/debug/alefsdb"
fi
if [[ ! -x "$BIN" ]]; then
  cargo build -q -p alefsdb
  BIN="$ROOT/target/debug/alefsdb"
fi

DATA="$(mktemp -d)"
trap 'rm -rf "$DATA"' EXIT
N="${1:-500}"

echo "data=$DATA n=$N"
"$BIN" mkdir --data "$DATA" --direct /bench >/dev/null
for i in $(seq 1 "$N"); do
  "$BIN" set --data "$DATA" --direct "/bench/k$i" --type string --value "v$i" >/dev/null
done
BEFORE=$(stat -c%s "$DATA/wal.log" 2>/dev/null || stat -f%z "$DATA/wal.log")
echo "wal_before=$BEFORE"
"$BIN" compact --data "$DATA" --direct >/dev/null
AFTER=$(stat -c%s "$DATA/wal.log" 2>/dev/null || stat -f%z "$DATA/wal.log")
echo "wal_after=$AFTER"
"$BIN" get --data "$DATA" --direct "/bench/k$N"
test "$AFTER" -lt "$BEFORE"
echo "ok"
