#!/usr/bin/env bash
# GUI smoke test under a virtual X display (development and CI, Linux). The
# product ships for Windows only (NG-XPLAT); this proves that the window and
# the wheel draw real content from real worker output, with no display server
# or GPU.
#
#   bash tools/smoke/gui-linux.sh [path to the sheaf binary] [output dir]
#
# Needs: Xvfb, ImageMagick (`import`, `convert`), Mesa software GL, libxkbcommon-x11,
# a built sheaf-worker beside the binary, and `bash tools/fetch-vendor` done.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
sheaf="${1:-$root/target/debug/sheaf}"
out="${2:-$(mktemp -d)}"
mkdir -p "$out"
work="$(mktemp -d)"
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$work"' EXIT

export DISPLAY=:77 LIBGL_ALWAYS_SOFTWARE=1 HOME="$work/home" XDG_CONFIG_HOME="$work/home/c" XDG_CACHE_HOME="$work/home/k"
mkdir -p "$HOME"
Xvfb :77 -screen 0 1400x900x24 >/dev/null 2>&1 &
sleep 2

cargo run -q -p fixture-gen --bin write-fixture -- "$work/a.pdf" 6
cargo run -q -p fixture-gen --bin write-fixture -- "$work/b.pdf" 3

# How much is on screen: the number of distinct colours (a blank or black screen has a handful).
colours() { convert "$1" -format %k info:; }

echo "== window with a file open"
"$sheaf" open "$work/a.pdf" &
win=$!
sleep 12
kill -0 "$win" || { echo "FAIL the window exited early"; exit 1; }
import -window root "$out/window.png"
n="$(colours "$out/window.png")"
echo "colours on screen: $n"
[ "$n" -gt 200 ] || { echo "FAIL the window drew almost nothing"; exit 1; }
kill "$win"; wait "$win" 2>/dev/null || true

echo "== action wheel on two PDFs"
printf '%s\n%s\n' "$work/a.pdf" "$work/b.pdf" > "$work/list.txt"
"$sheaf" wheel --at 600,400 --files "$work/list.txt" &
wheel=$!
sleep 9
kill -0 "$wheel" || { echo "FAIL the wheel exited early"; exit 1; }
import -window root "$out/wheel.png"
n="$(colours "$out/wheel.png")"
echo "colours on screen: $n"
[ "$n" -gt 100 ] || { echo "FAIL the wheel drew almost nothing"; exit 1; }
[ ! -e "$work/list.txt" ] || { echo "FAIL the file list was not consumed"; exit 1; }
kill "$wheel"; wait "$wheel" 2>/dev/null || true

echo "== headless action"
"$sheaf" run compress "$work/a.pdf" "$work/b.pdf" > "$out/run.txt"
[ "$(wc -l < "$out/run.txt")" -eq 2 ] || { echo "FAIL expected two outputs"; cat "$out/run.txt"; exit 1; }
[ -f "$work/a.compressed.pdf" ] || { echo "FAIL no compressed file"; exit 1; }
echo "smoke test passed; screenshots in $out"
