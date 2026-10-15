#!/usr/bin/env bash
# Reproducible, non-disruptive project checks. Pass --offline to use cached crates.
set -euo pipefail
repo_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo_dir"
sh -n resources/realand-session
bash -n scripts/install-session.sh scripts/check-vulkan.sh scripts/check-project.sh
python3 -m unittest discover -s scripts -p 'test_*.py'
cargo fmt --all -- --check
cargo test --workspace --locked "$@"
cargo clippy --workspace --all-targets --locked "$@" -- -D warnings
