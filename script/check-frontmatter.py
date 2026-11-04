#!/usr/bin/env python3
"""check-frontmatter.py — 校验 docs/ 各 Markdown 的 frontmatter 必填项齐全。

用法:
  python3 script/check-frontmatter.py
  python3 script/check-frontmatter.py --doc-root <docs目录>

- 扫描 docs/ 全部 *.md（递归，跳过 archive_docs_v* 存档目录）。
- 首行须为 `---`，且其后 50 行内出现闭合 `---`。
- 必填 4 项 type / status / summary / updated：存在且值非空。
- status 取值：通用文档限 active / draft / deprecated；ADR 决策篇可用 draft / accepted / superseded。
- 任何违规逐条列出并返回 exit code 1。
- 纯标准库，无第三方依赖（环境策略见 vibe-project-init references/scripts.md）。
"""

import argparse
import re
import sys
from pathlib import Path

REQUIRED = ("type", "status", "summary", "updated")
STATUS_VALUES = ("active", "draft", "deprecated", "accepted", "superseded")
KEY_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_-]*):\s*(.*)$")


def parse_frontmatter(lines: list[str]) -> dict[str, str] | None:
    """返回顶层键值映射；无 frontmatter 或未闭合返回 None。缩进的嵌套键忽略。"""
    if not lines or lines[0].strip() != "---":
        return None
    for idx in range(1, min(len(lines), 51)):
        if lines[idx].strip() == "---":
            fm: dict[str, str] = {}
            for ln in lines[1:idx]:
                m = KEY_RE.match(ln)
                if m and m.group(2).strip():
                    fm.setdefault(m.group(1), m.group(2).strip().strip("\"'"))
            return fm
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description="校验 docs/ frontmatter 必填项")
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

    issues: list[str] = []
    files = [
        p
        for p in sorted(args.doc_root.rglob("*.md"))
        if not any(part.startswith("archive_docs_v") for part in p.parts)
    ]
    for path in files:
        fm = parse_frontmatter(path.read_text(encoding="utf-8").splitlines())
        if fm is None:
            issues.append(f"VIOLATION {path}: 缺 frontmatter（--- 包裹头部，必填 {'/'.join(REQUIRED)}）")
            continue
        for key in REQUIRED:
            if not fm.get(key):
                issues.append(f"VIOLATION {path}: 必填项 {key} 缺失或值为空")
        if fm.get("status") and fm["status"] not in STATUS_VALUES:
            issues.append(f"VIOLATION {path}: status 值 `{fm['status']}` 不在 {'/'.join(STATUS_VALUES)}")

    if not issues:
        print(f"OK: {len(files)} file(s) frontmatter complete")
        return 0
    for it in issues:
        print(it)
    return 1


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main())
