#!/usr/bin/env bash
set -e

# Resolve paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
STAT_FILE="$REPO_ROOT/review-prompts/examples/review-stat.txt"

if [[ ! -f "$STAT_FILE" ]]; then
    echo "Error: File $STAT_FILE not found!"
    exit 1
fi

# Extract Message IDs
# Format in file: "msgid: <...>"
# We use grep to find lines starting with 'msgid:' and sed to extract the ID inside <>
mapfile -t MSG_IDS < <(grep "^msgid:" "$STAT_FILE" | sed -E 's/^msgid:[[:space:]]*<([^>]+)>/\1/')

if [[ ${#MSG_IDS[@]} -eq 0 ]]; then
    echo "No message IDs found in $STAT_FILE"
    exit 0
fi

echo "Found ${#MSG_IDS[@]} message IDs in $(basename "$STAT_FILE")."

PARENTS_FILE="$SCRIPT_DIR/review-parents.txt"
if [[ -f "$PARENTS_FILE" ]]; then
    mapfile -t PARENT_IDS < "$PARENTS_FILE"
    echo "Found ${#PARENT_IDS[@]} parent message IDs in $(basename "$PARENTS_FILE")."
    MSG_IDS+=("${PARENT_IDS[@]}")
fi

# Construct arguments
CMD_ARGS=("--ingest-only")
for id in "${MSG_IDS[@]}"; do
    CMD_ARGS+=("--message" "$id")
done

# Run sashiko
echo "Running sashiko ingestion..."
cd "$REPO_ROOT"
# Use cargo run to ensure we run the latest code in the repo
cargo run -- "${CMD_ARGS[@]}"
