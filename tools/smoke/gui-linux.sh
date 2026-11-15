#!/usr/bin/env bash
# GUI smoke test under a virtual X display (development and CI, Linux). The
# product ships for Windows only (NG-XPLAT); this proves that the window and
# the wheel draw real content from real worker output, with no display server
# or GPU.
#
#   bash tools/smoke/gui-linux.sh [path to the sheaf binary] [output dir]
#
# Needs: Xvfb, xdotool, ImageMagick (`import`, `convert`), Tesseract, Mesa software GL, libxkbcommon-x11,
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

echo "== scan dialog with a fake feeder"
# The fake scanner exists on development machines only (SHEAF_FAKE_SCANNER); on Windows the real WIA backend is used.
SHEAF_FAKE_SCANNER=1 "$sheaf" &
scan=$!
sleep 9
kill -0 "$scan" || { echo "FAIL the window exited early"; exit 1; }
xdotool mousemove 146 25 click 1            # toolbar: Scan
sleep 3
import -window root "$out/scan-dialog.png"
xdotool mousemove 565 309 click 1; sleep 1   # Source
xdotool mousemove 578 385 click 1; sleep 1   # Document feeder
xdotool mousemove 479 587 click 1            # Start scanning
sleep 8
import -window root "$out/scan-done.png"
# Read the job bar back: three sheets from the feeder are "3 pages".
convert "$out/scan-done.png" -crop 420x30+0+790 +repage -resize 300% -colorspace Gray -negate "$work/bar.png"
bar="$(tesseract "$work/bar.png" stdout --psm 7 2>/dev/null | tr -d '\n')"
echo "job bar reads: $bar"
case "$bar" in *"3 pages"*) ;; *) echo "FAIL expected the job bar to say 3 pages"; exit 1;; esac
kill "$scan"; wait "$scan" 2>/dev/null || true

echo "== presets: headless run and the dialog"
mkdir -p "$XDG_CONFIG_HOME/Sheaf/presets"
printf '{"compress":"Recommended","namePattern":"{source}.demo"}' > "$XDG_CONFIG_HOME/Sheaf/presets/Demo scan.json"
[ "$("$sheaf" presets)" = "Demo scan" ] || { echo "FAIL the preset is not listed"; exit 1; }
"$sheaf" preset "Demo scan" "$work/b.pdf" > "$out/preset.txt"
case "$(cat "$out/preset.txt")" in *"b.demo.pdf") ;; *) echo "FAIL unexpected preset output"; cat "$out/preset.txt"; exit 1;; esac
[ -f "$work/b.demo.pdf" ] || { echo "FAIL the preset wrote no file"; exit 1; }
"$sheaf" &
pre=$!
sleep 9
xdotool mousemove 446 25 click 1            # toolbar: Presets
sleep 3
import -window root "$out/presets-dialog.png"
convert "$out/presets-dialog.png" -crop 190x80+296+190 +repage -resize 300% -colorspace Gray -negate "$work/list.png"
listed="$(tesseract "$work/list.png" stdout --psm 6 2>/dev/null | tr -d '\n')"
echo "preset list reads: $listed"
case "$listed" in *Demo*) ;; *) echo "FAIL the dialog does not list the preset"; exit 1;; esac
kill "$pre"; wait "$pre" 2>/dev/null || true

echo "== Explorer-style selection: three processes, one merge"
agg="$work/agg"; mkdir -p "$agg"
for n in 1 2 3; do cargo run -q -p fixture-gen --bin write-fixture -- "$agg/m$n.pdf" "$n"; done
pids=()
for n in 1 2 3; do
  "$sheaf" merge --collect "$agg/m$n.pdf" > "$agg/out$n.txt" 2>"$agg/err$n.txt" &
  pids+=($!)
done
wait "${pids[@]}" || true
printed="$(cat "$agg"/out*.txt | grep -c . || true)"
echo "paths printed by the three processes: $printed"
[ "$printed" -eq 1 ] || { echo "FAIL expected exactly one process to act"; cat "$agg"/out*.txt "$agg"/err*.txt; exit 1; }
merged="$agg/m1.merged.pdf"
[ -f "$merged" ] || { echo "FAIL no merged file beside the first input"; ls "$agg"; exit 1; }
[ "$(stat -c %s "$merged")" -gt "$(stat -c %s "$agg/m3.pdf")" ] || { echo "FAIL the merge holds less than its largest input"; exit 1; }

echo "== a verb's card: progress and result at the pointer"
cp "$work/b.pdf" "$work/card.pdf"
"$sheaf" compress --card "$work/card.pdf" &
card=$!
sleep 12
kill -0 "$card" || { echo "FAIL the card exited before it could be read"; exit 1; }
import -window root "$out/card.png"
convert "$out/card.png" -crop 380x70+410+275 +repage -resize 300% -colorspace Gray -negate "$work/card-title.png"
title="$(tesseract "$work/card-title.png" stdout --psm 6 2>/dev/null | tr -d '\n')"
echo "card reads: $title"
case "$title" in *Done*) ;; *) echo "FAIL the card does not say Done"; exit 1;; esac
[ -f "$work/card.compressed.pdf" ] || { echo "FAIL the card's action made no file"; exit 1; }
kill "$card"; wait "$card" 2>/dev/null || true

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
