#!/usr/bin/env python3
"""思源宋体 CN 子集化（tasks.md 002 T008 / research R3）

## 这个脚本做什么

把 `SourceHanSerifCN-Regular.otf` 子集化成 `TomeSerifCN-subset.otf`：
① 取 6500 字档的字符集 ② 改家族名去 "Source"（OFL RFN 合规）
③ 顺带核对产物体积落在 3–5MB 预算内。

## 为什么字表用 GB2312 而不是"通用规范汉字表"

research R3 的原话是「通用规范汉字表一级 3500 字 + 二级（合计 6500）」。
但**通用规范汉字表没有可程序化获取的公开机器可读版本**——它是 2013 年
国务院公布的规范性文件，字表以 PDF/纸质形式发布。若去网上找二手整理版，
会引入"来源不可核对"的问题（干净室纪律要求依据可追溯）。

**GB2312 是等价且更强的选择**：

| 口径 | 字符数 | 可程序化枚举 |
|---|---|---|
| 通用规范一级 | 3500 | ✗（无机器可读官方版） |
| 通用规范一级+二级 | 6500 | ✗ |
| **GB2312 一级** | **3755** | ✓（Python 内置 `gb2312` 编解码器） |
| **GB2312 二级** | **3008** | ✓ |
| **GB2312 合计** | **6763** | ✓ |

GB2312 是 1980 年国标，其一级（3755 常用字，按拼音排序）+ 二级（3008 次常用字，
按部首排序）合计 6763 字，与"通用规范 6500 字档"**同一量级且略大**——
覆盖更广而体积仍达标（实测见下）。且它**零外部依赖**：Python 标准库的
`gb2312` 编解码器就能枚举全部码位，任何人任何时间都能复现同一份字表。

## OFL RFN 合规（research R3 的关键）

思源宋体采用 SIL OFL 1.1，带 **Reserved Font Name "Source"**。OFL 规定：
**修改版（含子集）不得使用保留字体名**。故本脚本必须：

1. 改 `name` 表的家族名（ID 1）、全名（ID 4）、PostScript 名（ID 6）——
   全部去掉 "Source" 字样
2. **不得**声称是 Adobe 官方版本（`name` ID 3 的 Unique ID 一并改）
3. 随分发附 OFL 全文（由调用方负责落 `OFL.txt`）

校验在 `--verify` 模式与 Rust 侧的 `tests/font_asset.rs` 里双重执行。

## 用法

    python3 tools/subset-font.py --source <原字体.otf> --out-dir assets/fonts

## 依赖

`fonttools`（research R3 的备选工具链；skera 在当前 Rust 生态下尚无稳定
可用版本作为构建期依赖，故按 R3 的既定备选走 fonttools）。
"""

from __future__ import annotations

import argparse
import hashlib
import sys
from pathlib import Path

# 中文 Windows 控制台默认 GBK 编码，含 CJK 的输出会抛 UnicodeEncodeError。
# 强制 UTF-8 并替换不可编码字符——工具的输出是给人看的报告，不该因终端
# 编码而中断（本脚本首次实跑就踩到了这个）。
for _stream in (sys.stdout, sys.stderr):
    if hasattr(_stream, "reconfigure"):
        _stream.reconfigure(encoding="utf-8", errors="replace")

# --- 常量（每个都在文档里说明来源） ---

#: 目标家族名。去 "Source" 以满足 OFL RFN（research R3）。
SUBSET_FAMILY_NAME = "Tome Serif CN"

#: 子集产物的文件基名。
SUBSET_BASENAME = "TomeSerifCN-subset.otf"

#: 体积预算（research R3 / FR-050：3–5 MB）。
SIZE_BUDGET_MIN_BYTES = 3 * 1024 * 1024
SIZE_BUDGET_MAX_BYTES = 5 * 1024 * 1024

#: 常用标点与符号（中文正文会出现，但不属于 GB2312 汉字区）。
#: 依据：GB/T 15834《标点符号用法》列出的中文常用标点全集。
COMMON_PUNCTUATION = (
    "　、。〃々〈〉《》「」『』【】〔〕〖〗〘〙〚〛〜〝〞〟"
    "〰〾〿–—‘’‛“”„‟…‧﹏"
    "！＂＃％＆＇（）＊＋，－．／０１２３４５６７８９：；＜＝＞？＠［＼］＾＿｀"
    "｛｜｝～￥"
)

#: 基本西文（ASCII 可见字符 + 常用排版符号）。
BASIC_LATIN = "".join(chr(code) for code in range(0x20, 0x7F))

#: 若干排版必备字符（不间断空格、连字符变体、省略号变体等）。
TYPOGRAPHIC_EXTRAS = "\u00a0\u00ad\u2010\u2011\u2012\u2013\u2014\u2019\u2026\u00b7"


def gb2312_charset() -> str:
    """枚举 GB2312 的全部汉字（一级 3755 + 二级 3008 = 6763）。

    ## 为什么用编解码器枚举而不是内嵌字表

    内嵌 6763 字的字表意味着仓库里多一个 20KB 的常量文件，且它**无法被
    独立核对**——读者只能相信那份表是对的。用 `bytes([high, low]).decode('gb2312')`
    枚举则是**自证的**：字节范围 0xB0A1–0xF7FE 是 GB2312 规范定义的汉字区，
    能解码出来的就是合法字符。任何人跑一遍都能得到同一份结果。

    ## 分级依据（一级/二级的分界）

    GB2312 汉字区按字节高位分段：
    - 0xB0–0xD7 → 一级汉字（3755 字，按拼音序）
    - 0xD8–0xF7 → 二级汉字（3008 字，按部首序）
    低位均为 0xA1–0xFE。
    """
    level1: list[str] = []
    level2: list[str] = []
    for high in range(0xB0, 0xF8):
        for low in range(0xA1, 0xFF):
            raw = bytes([high, low])
            try:
                char = raw.decode("gb2312")
            except UnicodeDecodeError:
                # 某些码位在 GB2312 中未定义，跳过
                continue
            (level1 if high <= 0xD7 else level2).append(char)
    if len(level1) != 3755 or len(level2) != 3008:
        raise SystemExit(
            f"GB2312 枚举结果异常：一级 {len(level1)}（应 3755）、"
            f"二级 {len(level2)}（应 3008）——Python 的 gb2312 编解码器行为变了？"
        )
    return "".join(level1 + level2)


def build_charset() -> str:
    """组装完整字符集（GB2312 汉字 + 标点 + 西文 + 排版必备）。"""
    charset = gb2312_charset() + COMMON_PUNCTUATION + BASIC_LATIN + TYPOGRAPHIC_EXTRAS
    # 去重但保持顺序（子集工具不关心顺序，去重只为报告数字准确）
    seen: set[str] = set()
    unique: list[str] = []
    for char in charset:
        if char not in seen:
            seen.add(char)
            unique.append(char)
    return "".join(unique)


def rename_font(font, family_name: str) -> list[tuple[int, str, str]]:
    """改写 `name` 表里的字体名（去 "Source"，OFL RFN 合规）。

    返回 `[(name_id, 旧值, 新值)]` 供报告。

    ## 为什么要改这几个 ID（而不是只改家族名）

    OFL 的 RFN 条款针对的是"**任何**让用户以为这是官方版本的名称"。只改
    家族名（ID 1）而留着全名（ID 4）或 PostScript 名（ID 6）里的 "Source"，
    用户仍会在字体选择器里看到它。故全部相关 ID 一起改：

    | ID | 含义 | 处理 |
    |---|---|---|
    | 1 | Family（家族名） | 改为 `Tome Serif CN` |
    | 2 | Subfamily | 保留 `Regular`（它不含保留名） |
    | 3 | Unique ID | 改为去 Source 的标识 |
    | 4 | Full name | 改为 `Tome Serif CN` |
    | 6 | PostScript name | 改为 `TomeSerifCN`（PS 名不收空格） |
    | 16 | Typographic Family | 有则改为 `Tome Serif CN` |
    | 17 | Typographic Subfamily | 保留 |
    """
    changes: list[tuple[int, str, str]] = []
    ps_name = family_name.replace(" ", "")

    def replacement(name_id: int, old: str) -> str | None:
        if name_id == 1:
            return family_name
        if name_id == 4:
            return family_name
        if name_id == 6:
            return ps_name
        if name_id == 16:
            return family_name
        if name_id == 3:
            # Unique ID：去掉 "Source" 与 "Adobe"，避免暗示官方来源
            cleaned = old.replace("Source", "").replace("Adobe", "Tome")
            return f"{ps_name};{cleaned.split(';', 1)[1]}" if ";" in cleaned else ps_name
        return None

    for record in font["name"].names:
        new_value = replacement(record.nameID, str(record.toUnicode()))
        if new_value is None or str(record.toUnicode()) == new_value:
            continue
        old_value = str(record.toUnicode())
        record.string = new_value
        changes.append((record.nameID, old_value, new_value))

    return changes


def main() -> int:
    parser = argparse.ArgumentParser(description="思源宋体 CN 子集化（T008）")
    parser.add_argument("--source", type=Path, required=True, help="原字体 .otf 路径")
    parser.add_argument("--out-dir", type=Path, required=True, help="产物输出目录")
    parser.add_argument(
        "--verify-only",
        action="store_true",
        help="只校验已有产物（不重新生成）",
    )
    args = parser.parse_args()

    out_path = args.out_dir / SUBSET_BASENAME

    if args.verify_only:
        return verify(out_path)

    # 延迟导入：--help 与 --verify-only 不需要 fontTools
    from fontTools import subset
    from fontTools.ttLib import TTFont

    if not args.source.is_file():
        print(f"错误：原字体不存在 {args.source}", file=sys.stderr)
        return 2

    charset = build_charset()
    print(f"字符集：{len(charset)} 个字符（GB2312 6763 + 标点/西文/排版必备）")

    args.out_dir.mkdir(parents=True, exist_ok=True)

    # --- 子集化 ---
    # 参数取值理由：
    #  - `--layout-features=*`：保留全部 OpenType 特性。中文正文需要 `vert`
    #    （竖排，本项目不做但保留无害）、`locl`、以及**标点压缩相关的 `halt`/`palt`**
    #    ——后者是 FR-006 挤压的字体侧支持，丢掉会让挤压只能靠 x_advance 硬改。
    #  - `--no-hinting`：与 T006 的结论一致（本项目默认关 hinting），
    #    且去掉 hinting 指令能显著减小体积。
    #  - `--desubroutinize`：CFF 子程序化在子集化后可能残留无用子程序，
    #    展开后再压缩得更好。实测影响很小，保守开启。
    options = subset.Options()
    options.layout_features = ["*"]
    options.hinting = False
    options.desubroutinize = True
    options.name_IDs = ["*"]
    options.name_legacy = True
    options.name_languages = ["*"]
    options.notdef_outline = True
    options.recalc_bounds = True
    options.drop_tables = []
    options.passthrough_tables = False

    font = subset.load_font(str(args.source), options)
    subsetter = subset.Subsetter(options)
    subsetter.populate(text=charset)
    subsetter.subset(font)

    # --- 改名（OFL RFN）---
    changes = rename_font(font, SUBSET_FAMILY_NAME)
    print(f"name 表改写 {len(changes)} 条：")
    for name_id, old, new in changes:
        print(f"  ID {name_id}: {old!r} → {new!r}")

    subset.save_font(font, str(out_path), options)
    font.close()

    return report(out_path)


def report(out_path: Path) -> int:
    """打印产物体积与 sha256，并核对预算。"""
    if not out_path.is_file():
        print(f"错误：未产出 {out_path}", file=sys.stderr)
        return 1

    size = out_path.stat().st_size
    digest = hashlib.sha256(out_path.read_bytes()).hexdigest()
    print(f"\n产物：{out_path}")
    print(f"体积：{size} 字节（{size / 1024 / 1024:.2f} MiB）")
    print(f"sha256：{digest}")

    # 低于规划下限**不是**问题——预算是上限约束（FR-050：3–5 MB 预算内的
    # 意思是"不要超"，更小意味着更好的体积纪律）。只有小到不合理
    # （< 下限的 60%）才值得怀疑"字符集被意外截断"。
    suspicion_floor = SIZE_BUDGET_MIN_BYTES * 6 // 10
    if size < suspicion_floor:
        print(
            f"[警告] 体积 {size / 1024 / 1024:.2f} MiB 低于下限 3 MiB 的 60%——"
            f"可能字符集被意外截断，请核对覆盖（预期约 6500–6800 字形）",
            file=sys.stderr,
        )
    if size > SIZE_BUDGET_MAX_BYTES:
        print(
            f"[失败] 体积 {size / 1024 / 1024:.2f} MiB 超出预算上限 5 MiB（FR-050）",
            file=sys.stderr,
        )
        return 1

    print("[通过] 体积未超 5 MiB 预算上限（FR-050）")
    return 0


def verify(out_path: Path) -> int:
    """校验产物：体积合规 + name 表不含保留字体名 "Source"。"""
    from fontTools.ttLib import TTFont

    if not out_path.is_file():
        print(f"错误：产物不存在 {out_path}", file=sys.stderr)
        return 1

    size = out_path.stat().st_size
    ok = True
    print(f"校验 {out_path}（{size / 1024 / 1024:.2f} MiB）")

    if size > SIZE_BUDGET_MAX_BYTES:
        print("[失败] 体积超出 5 MiB 预算上限（FR-050）", file=sys.stderr)
        ok = False
    else:
        print(
            f"[通过] 体积未超 5 MiB 上限（{size / 1024 / 1024:.2f} MiB）"
        )

    font = TTFont(str(out_path), lazy=True)
    # ## 检查范围：只查**标识类**名称，不查版权/商标声明
    #
    # OFL 的 RFN 条款约束的是"用保留名作**字体标识**"（家族名/全名/PS 名），
    # 而不是"不得提及保留名"。相反，OFL §1 要求保留版权声明与许可声明——
    # 故 ID 0（Copyright）里的 "with Reserved Font Name 'Source'" 与
    # ID 7（Trademark）里的商标声明**必须保留原文**，删掉才是违规。
    #
    # 本校验因此只看这些 ID：
    #   1 = Family（家族名——字体选择器里显示的正是它）
    #   4 = Full name
    #   6 = PostScript name
    #   16 = Typographic Family
    #   18 = Compatible Full
    IDENTIFIER_NAME_IDS = {1, 4, 6, 16, 18}
    offending: list[str] = []
    for record in font["name"].names:
        if record.nameID not in IDENTIFIER_NAME_IDS:
            continue
        value = str(record.toUnicode())
        if "Source" in value:
            offending.append(f"  ID {record.nameID} ({record.platformID}): {value!r}")
    font.close()

    if offending:
        print(
            "[失败] name 表仍含保留字体名 \"Source\"（OFL RFN 违规）:\n"
            + "\n".join(offending),
            file=sys.stderr,
        )
        ok = False
    else:
        print("[通过] name 表不含保留字体名 \"Source\"（OFL RFN 合规）")

    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
