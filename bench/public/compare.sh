#!/usr/bin/env bash
# Measures this tree and an earlier release under identical conditions: the
# same harness (this directory, copied into a worktree of the baseline), the
# same host, the same limits, and repetitions alternated between the two so
# drift on the host hits both alike.
#
#   bench/public/compare.sh v0.1.30
#
# Results: $OUT (default docs/performance/results/v<this version>) for this
# tree, and $OUT/baseline-<ref> for the baseline. `run.sh summarize` then adds
# the comparison to summary.json. Takes about 1.5 h with the defaults
# (REPS=3, EVAL_REPS=3); see run.sh for the other variables.
set -euo pipefail

BASE_REF="${1:?usage: compare.sh <baseline git ref>}"
ROOT="$(git rev-parse --show-toplevel)"
HERE="$ROOT/bench/public"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
OUT="${OUT:-$ROOT/docs/performance/results/v$VERSION}"
BASE_OUT="$OUT/baseline-$BASE_REF"
REPS="${REPS:-3}"
EVAL_REPS="${EVAL_REPS:-3}"

WT="$(mktemp -d)/baseline"
git -C "$ROOT" worktree add --detach "$WT" "$BASE_REF" >/dev/null
cleanup() { git -C "$ROOT" worktree remove --force "$WT" >/dev/null 2>&1 || true; }
trap cleanup EXIT
# The harness under test is this one, for both sides.
rm -rf "$WT/bench/public"
cp -R "$HERE" "$WT/bench/public"
rm -rf "$WT/bench/public/.work"

# Release builds of the Node addon on both sides (an existing debug build
# would make the comparison meaningless), and one image set per side.
for side in "$ROOT" "$WT"; do
  (cd "$side/sdks/nodejs" && npm ci --silent && npm run build --silent)
done
BENCH_IMAGE_TAG=candidate docker compose -f "$HERE/compose.yml" --profile tools build
BENCH_IMAGE_TAG=baseline docker compose -f "$WT/bench/public/compose.yml" --profile tools build

run_side() { # tree tag out rep
  (cd "$1" && OUT="$3" REP_START="$4" REPS=1 EVAL_REPS=1 BENCH_SKIP_BUILD=1 BENCH_IMAGE_TAG="$2" \
    BENCH_VERSION="$5" bench/public/run.sh "$6")
}

BASE_VERSION="${BASE_REF#v}"
for side in "$ROOT:candidate:$OUT:$VERSION" "$WT:baseline:$BASE_OUT:$BASE_VERSION"; do
  IFS=: read -r tree tag out ver <<<"$side"
  run_side "$tree" "$tag" "$out" 1 "$ver" environment
done
for r in $(seq 1 "$REPS"); do
  for side in "$ROOT:candidate:$OUT:$VERSION" "$WT:baseline:$BASE_OUT:$BASE_VERSION"; do
    IFS=: read -r tree tag out ver <<<"$side"
    [ "$r" -le "$EVAL_REPS" ] && run_side "$tree" "$tag" "$out" "$r" "$ver" eval
    run_side "$tree" "$tag" "$out" "$r" "$ver" server
    run_side "$tree" "$tag" "$out" "$r" "$ver" propagation
  done
done
echo "candidate: $OUT"
echo "baseline:  $BASE_OUT"
