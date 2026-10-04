#!/bin/sh
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
cargo build --manifest-path ../Cargo.toml --target-dir .build --bin statemcp --offline
statemcp_tmp=$(mktemp .statemcp.XXXXXX)
trap 'rm -f -- "$statemcp_tmp"' EXIT HUP INT TERM
cp .build/debug/statemcp "$statemcp_tmp"
chmod 755 "$statemcp_tmp"
mv -f "$statemcp_tmp" ./statemcp
