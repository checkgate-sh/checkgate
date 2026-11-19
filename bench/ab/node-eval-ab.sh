#!/usr/bin/env bash
# A/B of Node SDK evaluation latency between two builds of @checkgate/node,
# with the public harness (bench/public/sdk/eval.mjs), alternating runs.
#
#   BASE_SDK=/path/to/old/sdks/nodejs/index.js bench/ab/node-eval-ab.sh
#
# Build each SDK's addon first (`npm run build` in its sdks/nodejs). The
# candidate defaults to this checkout's SDK.
set -euo pipefail
REPO="$(git rev-parse --show-toplevel)"
BASE_SDK="${BASE_SDK:?set BASE_SDK to the baseline sdks/nodejs/index.js}"
CAND_SDK="${CAND_SDK:-$REPO/sdks/nodejs/index.js}"
RUNS="${RUNS:-3}"
OUT="${OUT:-$REPO/bench/ab/out/node-eval}"; mkdir -p "$OUT"
cd "$REPO/bench/public/sdk"
for i in $(seq 1 "$RUNS"); do
  for v in base cand; do
    sdk=$BASE_SDK; [ "$v" = cand ] && sdk=$CAND_SDK
    echo "$v run$i load=$(uptime | sed 's/.*load averages*: //')"
    CHECKGATE_SDK="$sdk" node eval.mjs --out "$OUT/$v-$i.json" > "$OUT/$v-$i.log" 2>&1
  done
done
python3 - "$OUT" "$RUNS" <<'PY'
import json, statistics as st, sys
out, runs = sys.argv[1], int(sys.argv[2])
res = {v: [json.load(open(f"{out}/{v}-{i}.json"))["cases"] for i in range(1, runs + 1)] for v in ("base", "cand")}
print(f"{'case':24}{'base median ns':>16}{'cand median ns':>16}{'speedup':>9}")
for case in res["base"][0]:
    b = st.median(r[case]["median_ns"] for r in res["base"]); c = st.median(r[case]["median_ns"] for r in res["cand"])
    print(f"{case:24}{b:>16.0f}{c:>16.0f}{b / c:>8.1f}x")
PY
