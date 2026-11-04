#!/usr/bin/env python3
"""check-links.py — 校验 docs/ 各 Markdown 的跨文件链接与站内锚点有效性。

用法:
  python3 script/check-links.py
  python3 script/check-links.py --doc-root <docs目录> [--cairn-root <cairn目录>] [--no-recursive]

- 扫描各根目录下全部 *.md（默认递归含子域目录；跳过 archive_docs_v* 存档目录）。
- 解析时跳过 fenced code block 与行内反引号代码，避免把示例代码当链接校验。
- 对 `](file.md#锚点)` 跨文件链接：按源文件所在目录解析相对路径，目标须存在
  （文件或目录；不存在 = BROKEN）；目标是 .md 时锚点与其标题生成的 GitHub slug
  比对，其余存在即可；目标在扫描根之外但确实存在时同样校验锚点（如 docs/ 指向根 README）。
- 对 `](#锚点)` 站内链接：锚点对本文件标题比对。
- http(s) / mailto 外链跳过。
- 任何 BROKEN / MISMATCH 逐条列出并返回 exit code 1（供 CI/本地检查）。
- 纯标准库，无第三方依赖（环境策略见 vibe-project-init references/scripts.md）。
"""

import argparse
import re
import sys
import unicodedata
from pathlib import Path

HEADING_RE = re.compile(r"^#{1,6} ")
LINK_RE = re.compile(r"\]\(([^)]+)\)")
TARGET_RE = re.compile(r"^([^#]*)#(.+)$")
INLINE_CODE_RE = re.compile(r"`[^`]*`")
EXTERNAL_RE = re.compile(r"^(https?://|mailto:)")


def github_slug(heading: str) -> str:
    out = ""
    for ch in heading.lower():
        if unicodedata.category(ch)[0] in ("L", "N") or ch in (" ", "-"):
            out += ch
    out = out.strip()
    while "  " in out:
        out = out.replace("  ", " ")
    return out.replace(" ", "-")


def headings_of(path: Path) -> list[str]:
    out = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if HEADING_RE.match(line):
            out.append(re.sub(r"^#+\s*", "", line))
    return out


def iter_md(root: Path, recursive: bool):
    if recursive:
        for p in sorted(root.rglob("*.md")):
            if any(part.startswith("archive_docs_v") for part in p.parts):
                continue
            yield p
    else:
        yield from sorted(root.glob("*.md"))


def main() -> int:
    parser = argparse.ArgumentParser(description="校验 docs/ Markdown 链接与锚点有效性")
    parser.add_argument(
        "--doc-root",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "docs",
        help="docs 目录（默认仓库根下 docs/）",
    )
    parser.add_argument(
        "--cairn-root",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "cairn",
        help="cairn 目录（默认仓库根下 cairn/；不存在则只查 docs/）",
    )
    parser.add_argument(
        "--no-recursive",
        action="store_true",
        help="只扫各根目录顶层 *.md，不递归子目录",
    )
    args = parser.parse_args()
    recursive = not args.no_recursive

    roots = [r for r in (args.doc_root, args.cairn_root) if r.is_dir()]
    if not roots:
        print(f"no markdown root found: {args.doc_root}")
        return 2

    sources: list[tuple[str, Path]] = []
    for root in roots:
        for p in iter_md(root, recursive):
            sources.append((str(p), p.resolve()))

    cache: dict[Path, list[str]] = {}
    issues: list[str] = []
    checked = 0

    for display, path in sources:
        lines = path.read_text(encoding="utf-8").splitlines()
        in_fence = False
        for i, line in enumerate(lines):
            if line.startswith("```"):
                # 围栏行只翻转状态，本身不提取链接
                in_fence = not in_fence
                continue
            if in_fence:
                continue
            # 先剔除行内反引号代码片段，避免把 `[文本](#锚点)` 之类当链接校验
            text = INLINE_CODE_RE.sub(" ", line)
            for m in LINK_RE.finditer(text):
                inner = m.group(1).strip()
                if EXTERNAL_RE.match(inner):
                    continue
                if "{{" in inner:
                    # 模板令牌（{{PLACEHOLDER}} 形态）：落地时才替换成真实路径/锚点，跳过
                    continue
                tm = TARGET_RE.match(inner)
                target, anchor = (tm.group(1).strip(), tm.group(2)) if tm else (inner, None)
                if target == "":
                    # 站内链接：锚点对本文件标题
                    if anchor is None:
                        continue
                    heads = cache.setdefault(path, headings_of(path))
                    target_display = display
                else:
                    # 跨文件链接：按源文件所在目录解析相对路径，目标须存在
                    resolved = (path.parent / target).resolve()
                    if not resolved.exists():
                        issues.append(f"BROKEN  {display}:{i + 1} -> {target}")
                        continue
                    if resolved.suffix.lower() != ".md":
                        # 目录 / 图片等非 Markdown 目标：存在即可
                        continue
                    target_display = str(resolved)
                    heads = cache.setdefault(resolved, headings_of(resolved))
                if anchor is None:
                    continue
                checked += 1
                if not any(github_slug(h) == anchor for h in heads):
                    issues.append(f"MISMATCH {display}:{i + 1} -> {target_display}#{anchor}")

    print(f"checked {checked} anchor link(s)")
    if not issues:
        print("OK: all links and anchors resolve")
        return 0
    for it in issues:
        print(it)
    return 1


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main())
