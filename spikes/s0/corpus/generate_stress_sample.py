#!/usr/bin/env python3
"""生成 T008 语料的 `11-large-stress.html`（大规模压力样本）。

## 为什么生成器要入仓库

该样本 227 KB / 300 章 / 14,000+ 节点——把它作为不透明的大文件塞进版本库，
会让"它到底测了什么"无从审查。生成器入仓库后，语料的**构造意图**是可见的：
每一处乱构造都在下面的代码里写明，读代码即可判断覆盖了什么。

## 为什么需要它

前 10 个样本合计仅 3.8 KB。用它们得出"Jsoup 与 html5ever 的 DOM 差异率 0%"
不足以排除"小样本恰好一致"。本样本把全部乱构造放大到接近真实网页规模
（300 章），使结论有量级上的说服力。

## 用法

    python3 spikes/s0/corpus/generate_stress_sample.py

输出覆盖 `spikes/s0/corpus/11-large-stress.html`。生成后需重跑 Java 侧导出：

    cd spikes/s0/tools
    "$JAVA_HOME/bin/java" -cp "lib/jsoup-1.23.2.jar;." JsoupDomDump ../corpus

## ⚠️ 构造上一个已踩的坑

**必须闭合 `<title>`**。第一版漏写 `</title>`，导致整篇文档落入 `<title>` 的
文本节点——两侧都被解析成"一个巨长的文本节点"，比对只剩 6 行且差异率显示 0%，
**却什么也没测到**（见 BASELINE.md §4.2.2 缺陷 6）。修改本脚本时请保持 head
结构完整。
"""

import random

# 固定种子：语料必须可复现，否则差异率数据无法被独立复核。
SEED = 20261009

# 章节数。取 300 使输出落在 ~220 KB——接近一个真实书源目录页的量级，
# 又不至于让 differential 跑得太慢。
CHAPTER_COUNT = 300

# 每章的段落数
PARAGRAPHS_PER_CHAPTER = 5

# 深嵌套层数（覆盖解析器的深度处理；Jsoup 的 maxDepth 默认为 512，此处留足余量）
DEEP_NESTING_DEPTH = 10


def build_html() -> str:
    """拼装压力样本。每段注释标明它引入的乱构造类型。"""
    random.seed(SEED)
    parts = []

    # head：刻意保持结构完整（见模块 docstring 的踩坑说明）
    parts.append('<!DOCTYPE html><html lang="zh-CN"><head>')
    parts.append('<meta charset="utf-8">')
    parts.append('<title>大规模压力样本</title>')
    # 样式与脚本：覆盖"内容以 DataNode 承载"的构造
    parts.append('<style>body{margin:0} .c{color:#333}</style>')
    parts.append(
        '<script>var cfg={"book":"压力测试","n":%d};</script>' % random.randint(100, 9999)
    )
    parts.append('</head><body>')

    for chapter in range(CHAPTER_COUNT):
        # 属性用引号（正常形态）
        parts.append('<div class="c" id="ch%d">' % chapter)
        # 未闭合的 <h3>：紧接 <p> 会被隐式闭合，这是真实网页最常见的乱构造
        parts.append('<h3>第%d章 标题含<b>粗体</b>与<em>斜体</em>' % (chapter + 1))

        for paragraph in range(PARAGRAPHS_PER_CHAPTER):
            # 实体滥用：合法实体（&nbsp; &amp; &copy）、缺分号的实体、裸和号
            parts.append('<p>段落%d 含 &nbsp; &amp; &copy 与裸和号 A&B' % paragraph)
            # 属性值不加引号（真实网页常见）
            parts.append('<span class="x" data-v=%d>行内文本' % paragraph)
            parts.append('<a href=ch%d.html>链接</a>' % (chapter + 1))

            # 每章中间插一个错位表格：触发 foster parenting
            # （缺 <tr> 的单元格 + 表格内的块级元素）
            if paragraph == 2:
                parts.append(
                    '<table><td>缺 tr 的单元格</td><tr><td>正常</td>'
                    '<div>表格内 div</div></tr></table>'
                )

        parts.append('</div>')

    # 深嵌套 + 未闭合的闭合标签（多出的 </div>）
    parts.append('<div>' * DEEP_NESTING_DEPTH)
    parts.append('<p>深嵌套</p>')
    parts.append('</div>' * DEEP_NESTING_DEPTH)

    parts.append('</body></html>')
    return ''.join(parts)


def main() -> None:
    import pathlib

    output_path = pathlib.Path(__file__).with_name('11-large-stress.html')
    html = build_html()
    output_path.write_text(html, encoding='utf-8')
    print(
        '已生成 %s：%d 字节，%d 章'
        % (output_path.name, len(html.encode('utf-8')), CHAPTER_COUNT)
    )


if __name__ == '__main__':
    main()
