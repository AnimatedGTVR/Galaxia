#!/bin/sh
# Run Galaxia as a window inside your current desktop.
# Set GALAXIA_TERM to choose what Alt+Enter opens.
set -e
cd "$(dirname "$0")/.."
nice -n 19 cargo build -j 2 -p galaxia-comp
exec ./target/debug/galaxia-comp "$@"
