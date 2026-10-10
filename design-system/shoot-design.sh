# ============================================================
# tome 设计稿逐屏截图
#
# 背景：Ardot 的 capture_screenshot / export_nodes 返回的媒体 URL
#       在本机被占位化（sign 全零），curl 恒 403，拿不到像素。
#       因此改用「本地渲染 + Edge 无头截图」这条不依赖远端凭证的路径，
#       渲染源 design-system/preview.html（数值与 MASTER.md 一致）。
#
# 用法：bash scripts/shoot-design.sh [输出目录]
# 注意：--screenshot 必须给 Windows 绝对路径（盘符形式），
#       相对路径与 /tmp 形式在 Edge 下会静默失败。
# ============================================================
set -u

# REPO_WIN 从脚本自身位置推导（原硬编码 WorkBuddy 树路径，2026-10-11 PM 移植时参数化）
REPO_WIN="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREVIEW="file:///$REPO_WIN/design-system/preview.html"
EDGE="/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"
OUT="${1:-$REPO_WIN/design-system/shots}"

mkdir -p "$OUT"
chmod +x "$EDGE" 2>/dev/null || true

# id:宽:高 —— 逐屏尺寸（窄窗阅读器是 820 宽）
SHOTS=(
  "shelf-day:1440:900"
  "shelf-night:1440:900"
  "reader-day:1440:900"
  "reader-spread:1440:900"
  "reader-narrow:820:900"
  "sources-day:1440:900"
  "sources-night:1440:900"
  "settings-day:1440:900"
  "settings-night:1440:900"
)

fail=0
for entry in "${SHOTS[@]}"; do
  id="${entry%%:*}"; rest="${entry#*:}"; w="${rest%%:*}"; h="${rest##*:}"
  png="$OUT/$id.png"

  # 每屏独立 user-data-dir，避免复用已有实例导致截图失败
  tmp_profile="$(mktemp -d)"

  "$EDGE" \
    --headless=new --disable-gpu --no-sandbox --hide-scrollbars \
    --user-data-dir="$tmp_profile" \
    --force-color-profile=srgb \
    --disable-lcd-text \
    --virtual-time-budget=9000 \
    --window-size="$w,$h" \
    --screenshot="C:/Users/FirenzeClaw/WorkBuddy/Worktrees/tome/main-b490be59/design-system/shots/$id.png" \
    "$PREVIEW?shot=$id" >/dev/null 2>&1

  rc=$?
  rm -rf "$tmp_profile" 2>/dev/null

  if [ -s "$png" ]; then
    size=$(stat -c%s "$png" 2>/dev/null || echo 0)
    echo "OK   $id  ${w}x${h}  ${size} bytes"
  else
    echo "FAIL $id  (rc=$rc)"
    fail=$((fail+1))
  fi
done

echo "---"
if [ "$fail" -eq 0 ]; then
  echo "全部成功，输出目录：$OUT"
else
  echo "$fail 屏失败"
fi
exit "$fail"
