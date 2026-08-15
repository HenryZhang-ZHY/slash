#!/usr/bin/env bash
#
# Rust line-coverage gate (task #53).
#
# Runs `cargo llvm-cov` across the workspace (excluding the CLI binary) and
# enforces a per-crate minimum line-coverage floor, so new code cannot regress
# coverage below the agreed bar. Also emits an HTML report under
# `target/llvm-cov/html` and an LCOV file at `target/llvm-cov/coverage.lcov`
# for local inspection / CI artifact upload.
#
# Floors are declared here in one place; tune them as coverage improves. The
# current values are the measured baselines from 2026-08-15 rounded to the
# nearest 5, so the gate is green today and tightens over time.
#
# Usage:
#   SLASH_TEST_DATABASE_URL=... scripts/coverage.sh

set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "error: cargo-llvm-cov is not installed (try: cargo install cargo-llvm-cov --locked)" >&2
  exit 1
fi

# Per-crate minimum line coverage, percent. Keys are crate directory names.
declare -A FLOOR=(
  [slash-command]=95
  [slash-config]=95
  [slash-core]=90
  [slash-server]=70
)

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
# Aggregate covered = total - missed per crate.
declare -A HIT=() TOTAL=()
while read -r file total missed rest; do
  crate="${file%%/*}"
  if [[ -n "${FLOOR[$crate]+x}" ]]; then
    HIT[$crate]=$(( ${HIT[$crate]:-0} + total - missed ))
    TOTAL[$crate]=$(( ${TOTAL[$crate]:-0} + total ))
  fi
done < <(grep -E '^slash-(command|config|core|server)/' target/llvm-cov/summary.txt)

status=0
echo ""
echo "== per-crate line coverage =="
for crate in slash-command slash-config slash-core slash-server; do
  total="${TOTAL[$crate]:-0}"
  hit="${HIT[$crate]:-0}"
  if [[ "$total" -eq 0 ]]; then
    echo "  $crate: no instrumented lines"
    continue
  fi
  pct=$(awk "BEGIN { printf \"%.1f\", 100 * $hit / $total }")
  floor="${FLOOR[$crate]}"
  ok="OK"
  if awk "BEGIN { exit !($hit * 100 < $floor * $total) }"; then
    ok="BELOW FLOOR ($floor%)"
    status=1
  fi
  echo "  $crate: ${pct}% ($hit/$total)  [floor ${floor}%]  $ok"
done

echo ""
echo "html report: target/llvm-cov/html"
echo "lcov report: target/llvm-cov/coverage.lcov"
exit "$status"
