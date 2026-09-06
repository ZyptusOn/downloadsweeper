#!/usr/bin/env sh
set -eu
cd "$(dirname "$0")/.."
export DS_DATA_DIR="${DS_DATA_DIR:-$PWD/.ds-data}"
export DS_CONFIG="${DS_CONFIG:-$PWD/config.toml}"
exec cargo run --locked -p ds-web -- --open "$@"
