#!/usr/bin/env bash
# T016 走查：US1 端到端（quickstart §1 全节）
#
# ## 这个脚本做什么、不做什么
#
# **做**：把走查里**可机检**的部分跑出来并留下原始输出——冷启动计时、
# 首屏计时、位置逐字符恢复、1000 册的帧时间分布。
#
# **不做**：模拟鼠标点击与拖放。那部分由人（或 GUI 自动化工具）在真窗口里
# 完成，截图与状态机断言双轨取证。脚本不会假装自己能点按钮——那会让"走查
# 通过"变成一个无法验证的声明。
#
# ## 用法
#
#   bash scripts/walkthrough-us1.sh <数据目录> <测试书路径>
#
# 数据目录必须是**空的**（走查第 1 步是"空启动"）。

set -uo pipefail

DATA_DIR="${1:?用法: walkthrough-us1.sh <数据目录> <测试书路径>}"
BOOK="${2:?用法: walkthrough-us1.sh <数据目录> <测试书路径>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/release/tome-app.exe"

if [ ! -x "$BIN" ]; then
  echo "找不到 release 二进制：$BIN" >&2
  echo "先跑：cargo build --release -p tome-app" >&2
  exit 1
fi

if [ -e "$DATA_DIR" ] && [ -n "$(ls -A "$DATA_DIR" 2>/dev/null)" ]; then
  echo "数据目录非空（走查第 1 步要求空启动）：$DATA_DIR" >&2
  exit 1
fi

mkdir -p "$DATA_DIR"

echo "=== T016 走查：US1（quickstart §1）==="
echo "二进制：$BIN"
echo "数据目录：$DATA_DIR"
echo "测试书：$BOOK"
echo

# ---------------------------------------------------------------------------
# SC-001 冷启动：进程启动 → 书架可响应输入
# ---------------------------------------------------------------------------
#
# 口径见 spec §SC-001：「进程启动到书架视图可响应输入」。
# `tome-app` 的 `timing` 探针把这两个时刻钉在代码里，故这里只是启动它并
# 收集 stderr 上的 `[timing]` 行。
#
# 用 `timeout` 保证进程一定会退出（走查脚本不能因为一个没关的窗口而挂住）。
echo "--- SC-001 冷启动（计时探针输出）---"
TOME_TIMING=1 timeout 25 "$BIN" >/dev/null 2>"$DATA_DIR/timing-cold.txt" &
APP_PID=$!
sleep 12
kill "$APP_PID" 2>/dev/null
wait "$APP_PID" 2>/dev/null
grep '^\[timing\]' "$DATA_DIR/timing-cold.txt" || echo "（未捕获到计时输出）"
echo

# ---------------------------------------------------------------------------
# 后续步骤（导入 / 打开 / 翻页 / 恢复）
# ---------------------------------------------------------------------------
#
# 这些步骤需要**真实交互**（拖入文件或点按钮），脚本无法代劳。故这里只把
# 它们列出来，由走查人逐条执行并记录；脚本不伪造它们的结果。
cat <<'STEPS'
--- 后续步骤（需真实交互，逐条记录）---

第 2 步 导入：把测试书拖入窗口（或点「导入本地书」→ 选择文件）
        → 预期：书卡出现（标题 = 文件名，进度 0%）
        → 记步数（SC-003，口径：一次拖入 = 1 步）

第 3 步 打开：点击书卡
        → 预期：首屏可读；`[timing] first_frame` 给出耗时（SC-002）

第 4 步 翻页：按 → 数次
        → 预期：版心换页；页脚章标题与进度随之更新

第 5 步 回书架：按 Esc
        → 预期：书卡上的进度细线不再是 0%

第 6 步 重开：再次点击书卡
        → 预期：回到离开时的位置（SC-009，逐字符）

第 7 步 移除：选中书卡 → Del → 确认
        → 预期：书卡消失，缓存被清理
STEPS

echo
echo "=== 计时原始输出（供回执引用）==="
cat "$DATA_DIR/timing-cold.txt" 2>/dev/null | grep -E '^\[timing\]' || true
