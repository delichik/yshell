#!/usr/bin/env bash
# Measures the RSS footprint of the compressed scrollback store.
#
# Creates a throwaway crate in the temp directory that depends on
# yshell-terminal, feeds 12000 lines into a 120x32 grid for two line limits
# (1000 and 10000), and prints the RSS delta of each run. Target: the 10000
# line run must stay <= 4 MiB (the pre-compression implementation measured
# ~8.6 MB for 1000 lines and ~77 MB for 10000 lines).
set -euo pipefail

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

script_dir=$(cd "$(dirname "$0")" && pwd)
repo_dir=$(cd "$script_dir/.." && pwd)
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/yshell-scrollback-measure.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT

fed_lines=${MEASURE_FED_LINES:-12000}
target_delta_kb=4096

export CARGO_TARGET_DIR="$work_dir/target"
mkdir -p "$work_dir/src"

cat > "$work_dir/Cargo.toml" <<EOF
[package]
name = "yshell-scrollback-measure"
version = "0.1.0"
edition = "2021"

[dependencies]
yshell-terminal = { path = "$repo_dir/crates/yshell-terminal" }
EOF

cat > "$work_dir/src/main.rs" <<'EOF'
use std::time::Instant;

use yshell_terminal::{TerminalGrid, TerminalParser};

fn rss_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.trim().trim_end_matches(" kB").trim().parse().unwrap_or(0);
        }
    }
    0
}

fn main() {
    let line_limit: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(10_000);
    let fed_lines: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(12_000);

    let mut grid = TerminalGrid::with_limits(120, 32, line_limit, 2_000_000);
    let mut parser = TerminalParser::new();
    let before = rss_kb();
    let start = Instant::now();
    for index in 0..fed_lines {
        let line = format!("line {index:>5} scrollback sample output with a few words\n");
        parser.advance(&mut grid, line.as_bytes());
    }
    let elapsed = start.elapsed();
    let after = rss_kb();

    println!(
        "limit={line_limit:>6} fed={fed_lines:>6} retained={:>6} time={elapsed:>9.2?} rss {before}KB -> {after}KB delta={}KB estimate={}KB",
        grid.scrollback_len(),
        after.saturating_sub(before),
        grid.scrollback_bytes_estimate() / 1024,
    );
}
EOF

echo "building measurement crate (first run compiles yshell-terminal) ..."
if ! cargo build --release --quiet --offline --manifest-path "$work_dir/Cargo.toml"; then
  cargo build --release --quiet --manifest-path "$work_dir/Cargo.toml"
fi

binary="$CARGO_TARGET_DIR/release/yshell-scrollback-measure"
results_file="$work_dir/results.txt"

{
  echo "== scrollback RSS measurement (120x32 grid, $fed_lines lines fed) =="
  "$binary" 1000 "$fed_lines"
  "$binary" 10000 "$fed_lines"
} | tee "$results_file"

delta_10k=$(sed -n 's/.*limit= *10000.*delta=\([0-9]*\)KB.*/\1/p' "$results_file")
echo "baseline (uncompressed, design doc): 1000 lines ~8600KB, 10000 lines ~77000KB"
echo "target: 10000 lines delta <= ${target_delta_kb}KB"

if [ -n "$delta_10k" ] && [ "$delta_10k" -le "$target_delta_kb" ]; then
  echo "PASS: 10000-line compressed scrollback costs ${delta_10k}KB"
else
  echo "FAIL: 10000-line run used ${delta_10k:-unknown}KB (limit ${target_delta_kb}KB)"
  exit 1
fi
