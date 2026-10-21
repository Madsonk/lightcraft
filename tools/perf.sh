#!/bin/bash
# Reproducible library-speed benchmark (lot E). See docs/performance.md.
#
#   tools/perf.sh prepare [N=300] [DEST=$HOME/lc-perf]   build the test set from CC0 raw samples
#   tools/perf.sh run [DIR=$HOME/lc-perf/set] [VISIBLE=48] [RUNS=3]
#
# prepare: needs `cargo xtask corpus --download` first (corpus/raw, CC0, sha256-pinned). It copies the raws to a
#   LOCAL disk (not a USB/NAS volume: that would measure the disk), then makes N unique files with APFS clones
#   (`cp -c`: no extra disk) plus a few trailing bytes each, so every file has its own content hash. 30 % of the
#   files are replaced by camera-sized JPEGs rendered from the raws with macOS `sips` (the other 70 % stay raw).
# run: builds perf_bench in release and runs it RUNS times, printing the median of each metric.
# Never point it at a personal library: it only reads, but the numbers are meant for the shared test set.
set -euo pipefail
cd "$(dirname "$0")/.."
cmd=${1:-run}
case "$cmd" in
prepare)
  N=${2:-300}
  DEST=${3:-$HOME/lc-perf}
  mkdir -p "$DEST/set" "$DEST/jpg"
  /bin/cp -f corpus/raw/* "$DEST"/
  files=($(cd "$DEST" && ls *.arw *.cr2 *.cr3 *.dng *.nef *.orf *.raf *.raw *.rw2 *.srw 2>/dev/null))
  for f in "${files[@]}"; do
    case "$f" in arw-sony-a7m3*|cr2-canon-7d*|nef-nikon-d7000*|orf-olympus*|raf-fuji-xt4*|rw2-panasonic-gh5*)
      sips -s format jpeg -s formatOptions 92 "$DEST/$f" --out "$DEST/jpg/${f%.*}.jpg" >/dev/null 2>&1 || true ;;
    esac
  done
  jpgs=("$DEST"/jpg/*.jpg)
  for ((i = 0; i < N; i++)); do
    n=$(printf %05d $i)
    if ((i % 10 >= 7)) && ((${#jpgs[@]} > 0)); then
      src=${jpgs[$((i % ${#jpgs[@]}))]}; ext=jpg
    else
      f=${files[$((i % ${#files[@]}))]}; src="$DEST/$f"; ext=${f##*.}
    fi
    /bin/cp -c "$src" "$DEST/set/IMG_$n.$ext"
    printf "lightcraft-perf-%06d" "$i" >>"$DEST/set/IMG_$n.$ext"
  done
  echo "made $N files in $DEST/set"
  ;;
run)
  DIR=${2:-$HOME/lc-perf/set}
  VIS=${3:-48}
  RUNS=${4:-3}
  cargo build --release -q -p lightcraft-engine --example perf_bench
  BIN=${CARGO_TARGET_DIR:-target}/release/examples/perf_bench
  tmp=$(mktemp -d)
  for ((r = 0; r < RUNS; r++)); do "$BIN" run "$DIR" "$VIS" >"$tmp/run$r.txt"; done
  python3 - "$tmp" <<'PY'
import sys, glob, statistics, collections
vals = collections.OrderedDict()
unit = {}
for f in sorted(glob.glob(sys.argv[1] + "/run*.txt")):
    for line in open(f):
        if line.startswith("#"):
            print(line.strip()); continue
        p = line.rstrip("\n").split("\t")
        if len(p) == 3:
            vals.setdefault(p[0], []).append(float(p[1])); unit[p[0]] = p[2]
for k, v in vals.items():
    print(f"{k}\t{statistics.median(v):.1f}\t{unit[k]}\t(min {min(v):.1f}, max {max(v):.1f}, n={len(v)})")
PY
  rm -rf "$tmp"
  ;;
*) echo "usage: tools/perf.sh prepare [N] [DEST] | run [DIR] [VISIBLE] [RUNS]" >&2; exit 2 ;;
esac
