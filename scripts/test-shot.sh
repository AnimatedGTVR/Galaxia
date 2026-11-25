#!/bin/sh
# Start Galaxia with a few test windows and save a Galaxia-only screenshot to target/shots/.
set -e
cd "$(dirname "$0")/.."
nice -n 19 cargo build -j 2 -p galaxia-comp
OUT="$PWD/target/shots"; mkdir -p "$OUT"
GALAXIA_SCREENSHOT_AFTER=${DELAY:-12} GALAXIA_SCREENSHOT_DIR="$OUT" RUST_LOG=info,smithay=warn NO_COLOR=1 \
  ./target/debug/galaxia-comp > "$OUT/gx.log" 2>&1 &
PID=$!
sleep 2
SOCK=$(grep -o 'socket="wayland-[0-9]*"' "$OUT/gx.log" | head -1 | cut -d'"' -f2)
G="ghostty --gtk-single-instance=false --background-opacity=0.55"
for app in "$G" "env QT_QPA_PLATFORM=wayland qt6ct" "$G"; do
  WAYLAND_DISPLAY=$SOCK $app >/dev/null 2>&1 &
  sleep 2
done
sleep $(( ${DELAY:-12} - 4 ))
kill $PID
grep -E "screenshot|WARN|ERROR" "$OUT/gx.log" || true
