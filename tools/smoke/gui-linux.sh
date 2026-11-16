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
# Every step but the welcome one starts from a configuration that has already seen the welcome window.
mkdir -p "$XDG_CONFIG_HOME/Sheaf"; touch "$XDG_CONFIG_HOME/Sheaf/welcome-seen"
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
xdotool mousemove 565 290 click 1; sleep 1   # Source
xdotool mousemove 578 366 click 1; sleep 1   # Document feeder
xdotool mousemove 479 608 click 1            # Start scanning
sleep 8
import -window root "$out/scan-done.png"
# Read the job bar back: three sheets from the feeder are "3 pages".
convert "$out/scan-done.png" -crop 420x30+0+755 +repage -resize 300% -colorspace Gray -negate "$work/bar.png"
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

echo "== repair: a PDF with its tail cut off"
python3 - "$work/a.pdf" "$work/cut.pdf" <<'PY'
import sys
b = open(sys.argv[1], "rb").read()
open(sys.argv[2], "wb").write(b[:-80])
PY
repaired="$("$sheaf" repair "$work/cut.pdf" 2>"$out/repair-err.txt")"
case "$repaired" in *"cut.repaired.pdf") ;; *) echo "FAIL unexpected repair output: $repaired"; cat "$out/repair-err.txt"; exit 1;; esac
grep -q "6 pages recovered" "$out/repair-err.txt" || { echo "FAIL the repair did not say how many pages came back"; cat "$out/repair-err.txt"; exit 1; }
[ -f "$work/cut.repaired.pdf" ] || { echo "FAIL no repaired file"; exit 1; }

echo "== info: set, read back, strip"
edited="$("$sheaf" info "$work/b.pdf" --set "title=Smoke test" --set "author=Sheaf" 2>"$out/info-set.txt")"
case "$edited" in *"b.edited.pdf") ;; *) echo "FAIL unexpected info output: $edited"; cat "$out/info-set.txt"; exit 1;; esac
grep -q "title: Smoke test" "$out/info-set.txt" || { echo "FAIL the title was not written"; cat "$out/info-set.txt"; exit 1; }
"$sheaf" info "$edited" 2>"$out/info-read.txt" >/dev/null
grep -q "author: Sheaf" "$out/info-read.txt" || { echo "FAIL the author was not read back"; cat "$out/info-read.txt"; exit 1; }
cleaned="$("$sheaf" info "$edited" --strip 2>"$out/info-strip.txt")"
grep -q "xmp: no" "$out/info-strip.txt" || { echo "FAIL strip left the XMP packet"; cat "$out/info-strip.txt"; exit 1; }
! grep -aq "Smoke test" "$cleaned" || { echo "FAIL the title is still in the stripped file"; exit 1; }

echo "== wheel options: the safe triangle keeps a flyout open on the way to it"
printf '%s\n%s\n' "$work/a.pdf" "$work/b.pdf" > "$work/aim-list.txt"
"$sheaf" wheel --at 600,400 --files "$work/aim-list.txt" &
aimwheel=$!
sleep 9
kill -0 "$aimwheel" || { echo "FAIL the wheel exited early"; exit 1; }
blue() { convert "$1" -format '%[fx:int(255*p{805,293}.b)-int(255*p{805,293}.r)]' info:; }
xdotool mousemove 695 330; sleep 0.5            # low in the Compress slot
for i in 1 2 3 4 5 6 7 8 9 10 11 12; do         # across the gap beside the ring to "Smallest"
  xdotool mousemove $((695 + i*185/12)) $((330 - i*37/12)); sleep 0.03
done
sleep 0.4
import -window root "$out/wheel-aimed.png"
diff="$(blue "$out/wheel-aimed.png")"
echo "blue over red at the Smallest option: $diff"
[ "$diff" -gt 60 ] || { echo "FAIL the flyout closed on the way: the safe triangle did not hold"; exit 1; }
# Standing still in the gap is not aiming: the flyout goes.
xdotool mousemove 20 20; sleep 0.5
xdotool mousemove 695 330; sleep 0.4
for i in 1 2 3 4 5 6; do xdotool mousemove $((695 + i*92/6)) $((330 - i*19/6)); sleep 0.03; done
sleep 0.9
import -window root "$out/wheel-stopped.png"
diff="$(blue "$out/wheel-stopped.png")"
[ "$diff" -lt 20 ] || { echo "FAIL the flyout stayed open for a pointer that stopped"; exit 1; }
kill "$aimwheel"; wait "$aimwheel" 2>/dev/null || true

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

echo "== messages and help: welcome on a first start, an error with details and a report, the help center"
fresh="$work/fresh"; mkdir -p "$fresh"
XDG_CONFIG_HOME="$fresh" "$sheaf" &
wel=$!
sleep 9
import -window root "$out/welcome.png"
convert "$out/welcome.png" -crop 400x40+373+195 +repage -resize 300% -colorspace Gray -negate "$work/w.png"
read_w="$(tesseract "$work/w.png" stdout --psm 7 2>/dev/null | tr -d '\n')"
echo "welcome reads: $read_w"
case "$read_w" in *Welcome*) ;; *) echo "FAIL the welcome window did not appear on a first start"; exit 1;; esac
xdotool mousemove 850 607 click 1; sleep 2   # Get started
[ -f "$fresh/Sheaf/welcome-seen" ] || { echo "FAIL the welcome window does not remember it was seen"; exit 1; }
kill "$wel"; wait "$wel" 2>/dev/null || true

# The run above was killed, not closed: its mark is still there, so the next start says so.
echo "== a run that did not close properly is noticed at the next start"
"$sheaf" open "$work/a.pdf" &
unc=$!
sleep 9
import -window root "$out/unclean.png"
convert "$out/unclean.png" -crop 280x30+965+686 +repage -resize 300% -colorspace Gray -negate "$work/u.png"
read_u="$(tesseract "$work/u.png" stdout --psm 7 2>/dev/null | tr -d '\n')"
echo "notice reads: $read_u"
case "$read_u" in *properly*) ;; *) echo "FAIL the notice about the last run did not appear"; exit 1;; esac
kill "$unc"; wait "$unc" 2>/dev/null || true
# The steps below click on fixed places, so they start without a notice about earlier killed runs
# (every step above ended its window with a kill, in this folder and the usual one).
rm -f "$fresh/Sheaf/running.mark" "$XDG_CONFIG_HOME/Sheaf/running.mark"

echo "not a pdf" > "$work/bad.pdf"
"$sheaf" open "$work/bad.pdf" &
err=$!
sleep 10
import -window root "$out/error-toast.png"
xdotool mousemove 1084 731 click 1; sleep 2   # Details, in the error toast
import -window root "$out/error-details.png"
convert "$out/error-details.png" -crop 470x40+403+330 +repage -resize 300% -colorspace Gray -negate "$work/e.png"
read_e="$(tesseract "$work/e.png" stdout --psm 7 2>/dev/null | tr -d '\n')"
echo "error details read: $read_e"
case "$read_e" in *"not available"*) ;; *) echo "FAIL the error details did not open"; exit 1;; esac
xdotool mousemove 543 472 click 1; sleep 3   # Report a problem
import -window root "$out/report.png"
convert "$out/report.png" -crop 580x460+350+130 +repage -resize 200% -colorspace Gray -negate "$work/r.png"
read_r="$(tesseract "$work/r.png" stdout --psm 6 2>/dev/null | tr '\n' ' ')"
echo "report reads: $read_r"
case "$read_r" in *"Report a problem"*) ;; *) echo "FAIL the report window did not open"; exit 1;; esac
case "$read_r" in *Unsupported*) ;; *) echo "FAIL the report does not list the error"; exit 1;; esac
xdotool mousemove 889 666 click 1; sleep 1   # Close
xdotool mousemove 1214 25 click 1; sleep 3   # Help
import -window root "$out/help.png"
convert "$out/help.png" -crop 740x540+20+20 +repage -resize 200% -colorspace Gray -negate "$work/h.png"
read_h="$(tesseract "$work/h.png" stdout --psm 6 2>/dev/null | tr '\n' ' ')"
echo "help reads: $read_h"
case "$read_h" in *"Getting started"*) ;; *) echo "FAIL the help center did not open"; exit 1;; esac
kill "$err"; wait "$err" 2>/dev/null || true

echo "== headless action"
"$sheaf" run compress "$work/a.pdf" "$work/b.pdf" > "$out/run.txt"
[ "$(wc -l < "$out/run.txt")" -eq 2 ] || { echo "FAIL expected two outputs"; cat "$out/run.txt"; exit 1; }
[ -f "$work/a.compressed.pdf" ] || { echo "FAIL no compressed file"; exit 1; }
echo "smoke test passed; screenshots in $out"
