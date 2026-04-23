#!/usr/bin/env bash
set -euo pipefail

echo "Cleaning up..."
rm -f sashiko.db sashiko.db.bak

# In CI, we use the release binary if it exists
SASHIKO_BIN="./target/release/sashiko"
if [ ! -f "$SASHIKO_BIN" ]; then
    SASHIKO_BIN="cargo run --bin sashiko --"
fi

echo "Starting server..."
$SASHIKO_BIN --no-ai &
SERVER_PID=$!

# Ensure server is killed on exit
trap 'kill $SERVER_PID || true' EXIT

sleep 10

exit 0