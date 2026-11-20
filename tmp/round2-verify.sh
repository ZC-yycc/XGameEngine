#!/usr/bin/env bash
set -u
cd /e/GitHubClone/XGameEngine/xgc || exit 1
C=/c/Users/Administrator/.cargo/bin/cargo.exe
OUT=/e/GitHubClone/XGameEngine/tmp/round2
mkdir -p "$OUT"

echo "== test =="
CARGO_BUILD_JOBS=1 "$C" test --workspace --offline > "$OUT/test.log" 2>&1
echo "TEST_EXIT=$?"
grep -h "test result:" "$OUT/test.log" | awk '{for(i=1;i<=NF;i++){if($i=="passed;")p+=$(i-1)}} END{print "PASSED_TOTAL="p}'
echo "FAILED_LINES=$(grep -c 'test result: FAILED' "$OUT/test.log" || true)"

echo "== clippy =="
CARGO_BUILD_JOBS=1 "$C" clippy --workspace --all-targets --offline -- -D warnings > "$OUT/clippy.log" 2>&1
echo "CLIPPY_EXIT=$?"
tail -3 "$OUT/clippy.log"

echo "== xtask =="
"$C" run -q -p xgc-xtask --offline -- check
echo "XTASK_EXIT=$?"

echo "== cli check =="
"$C" run -q -p xgc-cli --offline -- check
echo "CLICHECK_EXIT=$?"

echo "== run 60 =="
"$C" run -q -p xgc-cli --offline -- run 60
echo "RUN_EXIT=$?"

echo "== render-frame =="
"$C" run -q -p xgc-cli --offline -- render-frame ../tmp/showcase.json "$OUT/ssao-150.ppm" ../projects/XGame 1280 720 150
echo "RENDER_EXIT=$?"
sha256sum "$OUT/ssao-150.ppm"
