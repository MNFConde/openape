#!/usr/bin/env python3
"""check-markers.py — 校验 docs/ 方案状态标记（行内简记 + 块记）一致性。

用法:
  python3 script/check-markers.py
  python3 script/check-markers.py --doc-root <docs目录>

扫描 docs/ 全部 *.md（递归，跳过 archive_docs_v* 存档目录），检查:

1. 块记（独立小标题 + admonition callout）：
   - 每个 callout 上方最近的非空行为 `#` 标题（锚点，标题不含标记）
   - callout 标题行 `> [!TYPE] 【标签】 方案标题` 的类型↔标签映射正确：
     当前→IMPORTANT、备选→NOTE、已否决→CAUTION、已替换→WARNING
   - 备选/已否决/已替换：块内第一内容行以 `触发：` / `原因：` 开头，随后一行 `> ` 空行
   - 当前：无触发/原因行，标题行后直接 `> ` 空行
2. 行内简记 = 链接：标记必须以 `[【标签】 方案标题](#锚点)` 形式出现；
   既不在链接内、也不在块记标题行内的裸标签 → 报错；
   行内反引号内的标签视为代码示例，跳过（规则文档解释标记语法时用反引号包裹）

退出码：0 = 全部通过；1 = 存在违规（逐条列出）
纯标准库，无第三方依赖（环境策略见 vibe-project-init references/scripts.md）。
"""

import argparse
import re
import sys
from pathlib import Path

LABELS = ("【当前】", "【备选】", "【已否决】", "【已替换】")
TYPE_TO_LABEL = {
    "IMPORTANT": "【当前】",
    "NOTE": "【备选】",
    "CAUTION": "【已否决】",
    "WARNING": "【已替换】",
}
PREFIX_BY_LABEL = {"【备选】": "触发：", "【已否决】": "原因：", "【已替换】": "原因："}

FENCE_RE = re.compile(r"^\s*(```|~~~)")
BLOCK_TITLE_RE = re.compile(r"^> \[!(IMPORTANT|NOTE|CAUTION|WARNING)\] (【当前】|【备选】|【已否决】|【已替换】) (.+)$")
INLINE_LINK_RE = re.compile(r"\[(【当前】|【备选】|【已否决】|【已替换】)\s+[^\]\n]+\]\([^)]*\)")
INLINE_CODE_RE = re.compile(r"`[^`]*`")
LABEL_RE = re.compile("|".join(re.escape(l) for l in LABELS))
EMPTY_QUOTE_RE = re.compile(r"^>\s*$")


def check_block(lines: list[str], i: int, bt: re.Match, prev_nonblank: str) -> list[str]:
    type_, label, title = bt.group(1), bt.group(2), bt.group(3)
    out = []
    if TYPE_TO_LABEL.get(type_) != label:
        out.append(f"callout [!{type_}] 与标签 {label} 不匹配（应为 {TYPE_TO_LABEL[type_]}）")
    if not prev_nonblank.lstrip().startswith("#"):
        out.append(f"块记缺少独立小标题作锚点（{label} {title}）")

    prefix = PREFIX_BY_LABEL.get(label)
    j = i + 1
    if prefix:
        if j >= len(lines) or not lines[j].startswith(f"> {prefix}"):
            out.append(f"{label} 块第一内容行应为 `> {prefix}…`")
            return out
        j += 1
    if j >= len(lines) or not EMPTY_QUOTE_RE.match(lines[j]):
        out.append(f"{label} 块在标题行/触发原因行后缺 `> ` 空行")
    return out


def check_file(path: Path, issues: list[str]) -> None:
    lines = path.read_text(encoding="utf-8").splitlines()
    prev_nonblank = ""
    in_fence = False

    for i, line in enumerate(lines):
        if FENCE_RE.match(line):
            in_fence = not in_fence
            prev_nonblank = line
            continue
        if in_fence:
            continue

        bt = BLOCK_TITLE_RE.match(line)
        if bt:
            for it in check_block(lines, i, bt, prev_nonblank):
                issues.append(f"{path}:{i + 1}: {it}")
        else:
            # 剔除行内反引号代码片段：规则文档解释标记语法时以 `【标签】` 形式举例，不算使用
            text = INLINE_CODE_RE.sub(" ", line)
            link_spans = [m.span() for m in INLINE_LINK_RE.finditer(text)]
            for m in LABEL_RE.finditer(text):
                if not any(s <= m.start() and m.end() <= e for s, e in link_spans):
                    issues.append(f"{path}:{i + 1}: 裸标签 {m.group()} 未用行内简记 [【标签】 方案标题](#锚点)")

        if line.strip():
            prev_nonblank = line


def main() -> int:
    parser = argparse.ArgumentParser(description="校验 docs/ 方案状态标记一致性")
    parser.add_argument(
        "--doc-root",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "docs",
        help="docs 目录（默认仓库根下 docs/）",
    )
    args = parser.parse_args()
    if not args.doc_root.is_dir():
        print(f"no markdown root found: {args.doc_root}")
        return 2

    files = [
        p
        for p in sorted(args.doc_root.rglob("*.md"))
        if not any(part.startswith("archive_docs_v") for part in p.parts)
    ]
    issues: list[str] = []
    for path in files:
        check_file(path, issues)

    if not issues:
        print(f"OK: markers consistent ({len(files)} file(s))")
        return 0
    for it in issues:
        print("VIOLATION " + it)
    return 1


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main())
