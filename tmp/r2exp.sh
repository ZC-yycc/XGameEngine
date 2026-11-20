#!/usr/bin/env bash
# 第二轮复核实验（只读仓库；临时产物只在 /tmp）
set -u
BIN=/e/GitHubClone/XGameEngine/xgc/target/debug/xgc.exe
SB=$(mktemp -d /tmp/r2-script-XXXXXX)
cp /tmp/xgc-trace-script-20996-runs/world.json "$SB/world.json"
mkdir -p "$SB/assets"
W="$SB/world.json"
A="$SB/assets"
OUT="$SB/out.txt"
ERR="$SB/err.txt"

run() {
  echo "### $*"
  "$@" >"$OUT" 2>"$ERR"
  echo "exit=$?"
  echo "--stdout--"; cat "$OUT"
  echo "--stderr--"; cat "$ERR"
}

echo "================ A. 好脚本 trace 3 帧"
printf 'function update(ctx)\n  set_pos(ctx, 5.0, 0.0, 0.0)\nend\n' > "$A/move.lua"
run "$BIN" trace "$W" 3 "$A"

echo "================ B. 语法错 trace 3 帧"
printf 'function update(ctx)\n  this is not lua\nend\n' > "$A/move.lua"
run "$BIN" trace "$W" 3 "$A"

echo "================ C. update 运行期 error() trace 3 帧"
printf 'function update(ctx)\n  error("boom")\nend\n' > "$A/move.lua"
run "$BIN" trace "$W" 3 "$A"

echo "================ D. start 里 error() trace 3 帧"
printf 'function start(ctx)\n  error("start boom")\nend\nfunction update(ctx)\n  set_pos(ctx, 5.0, 0.0, 0.0)\nend\n' > "$A/move.lua"
run "$BIN" trace "$W" 3 "$A"

echo "================ E. 不存在资产根 + 有脚本世界 trace 3 帧"
run "$BIN" trace "$W" 3 /tmp/r2-does-not-exist

echo "================ F. 坏脚本 run-scripts 3 帧"
printf 'function update(ctx)\n  this is not lua\nend\n' > "$A/move.lua"
run "$BIN" run-scripts "$W" "$A" 3

echo "================ G. 不存在资产根 run-scripts 3 帧"
run "$BIN" run-scripts "$W" /tmp/r2-does-not-exist 3

echo "================ H. showcase 真实世界 trace / run-scripts"
S=/e/GitHubClone/XGameEngine/tmp/showcase.json
P=/e/GitHubClone/XGameEngine/projects/XGame
echo "### plain trace 3"; "$BIN" trace "$S" 3 2>&1; echo "exit=$?"
echo "### assets trace 3"; "$BIN" trace "$S" 3 "$P" 2>&1; echo "exit=$?"
echo "### run-scripts 4"; "$BIN" run-scripts "$S" "$P" 4 2>&1; echo "exit=$?"
echo "### assets trace 4 summary"; "$BIN" trace "$S" 4 "$P" 2>&1 | tail -1

rm -rf "$SB"
echo R2EXP_DONE
