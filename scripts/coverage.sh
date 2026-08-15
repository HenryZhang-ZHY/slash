#!/usr/bin/env bash
#
# Rust line-coverage report (task #53).
#
# Runs `cargo llvm-cov` across the workspace (excluding the CLI binary),
# prints per-crate line coverage as information (report-only, no hard
# threshold per @HenryZhang's decision), and emits an HTML report under
# `target/llvm-cov/html` plus an LCOV file at `target/llvm-cov/coverage.lcov`
# for local inspection / CI artifact upload.
#
# Usage:
#   SLASH_TEST_DATABASE_URL=... scripts/coverage.sh

set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "error: cargo-llvm-cov is not installed (try: cargo install cargo-llvm-cov --locked)" >&2
  exit 1
fi

echo "== measuring line coverage (workspace, excluding slash-cli) =="
mkdir -p target/llvm-cov
cargo llvm-cov \
  --workspace \
  --exclude slash-cli \
  --summary-only \
  --no-cfg-coverage >target/llvm-cov/summary.txt
cargo llvm-cov \
  --workspace \
  --exclude slash-cli \
  --no-cfg-coverage \
  --output-dir target/llvm-cov \
  --html >/dev/null
cargo llvm-cov \
  --workspace \
  --exclude slash-cli \
  --no-cfg-coverage \
  --lcov --output-path target/llvm-cov/coverage.lcov >/dev/null

# The per-file summary rows are:  <file> <total lines> <missed lines> <pct> ...
# Aggregate covered = total - missed per crate, purely for the informational print.
declare -A HIT=() TOTAL=()
while read -r file total missed rest; do
  crate="${file%%/*}"
  HIT[$crate]=$(( ${HIT[$crate]:-0} + total - missed ))
  TOTAL[$crate]=$(( ${TOTAL[$crate]:-0} + total ))
done < <(grep -E '^slash-(command|config|core|server)/' target/llvm-cov/summary.txt)

echo ""
echo "== per-crate line coverage (report-only) =="
for crate in slash-command slash-config slash-core slash-server; do
  total="${TOTAL[$crate]:-0}"
  hit="${HIT[$crate]:-0}"
  if [[ "$total" -eq 0 ]]; then
    echo "  $crate: no instrumented lines"
    continue
  fi
  pct=$(awk "BEGIN { printf \"%.1f\", 100 * $hit / $total }")
  echo "  $crate: ${pct}% ($hit/$total)"
done

echo ""
echo "html report: target/llvm-cov/html"
echo "lcov report: target/llvm-cov/coverage.lcov"
