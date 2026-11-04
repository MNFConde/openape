#!/usr/bin/env python3
"""check-commit-msg.py — 校验 git 提交信息是否符合 Conventional Commits 格式。

用法:
  python3 script/check-commit-msg.py <提交信息文件>

由 .githooks/commit-msg 钩子调用；仅校验本地提交，绝不自动推送。
不符合时逐条输出原因并返回 exit code 1。
纯标准库，无第三方依赖（环境策略见 vibe-project-init references/scripts.md）。
"""

import re
import sys
import unicodedata
from pathlib import Path

TYPES = {"feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert"}
SUBJECT_RE = re.compile(r"^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\([^()\s]+\))?:\s+(\S.*)$")
CLOSES_RE = re.compile(r"^Closes\s+#\d+(,\s*#\d+)*$")
EXEMPT_PREFIX = ("Merge ", "Revert ", "fixup! ", "squash! ")
MAX_LINE = 72
MAX_BODY_LINES = 20
FOOTER_PREFIX = ("BREAKING CHANGE:", "BREAKING-CHANGE:", "Closes")


def display_width(s: str) -> int:
    """终端显示列宽：East Asian Width 为 W/F（全角/CJK）计 2 列，其余计 1 列。"""
    return sum(2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1 for ch in s)


def is_footer_block(lines: list[str], i: int) -> bool:
    """第 i 行起是否处于 footer 段（footer 头行及其后续行）——不计入正文行数。"""
    for j in range(i, -1, -1):
        ln = lines[j]
        if any(ln.startswith(p) for p in FOOTER_PREFIX):
            return True
        if ln == "" and j < i:
            return False
    return False


def check(msg: str) -> list[str]:
    lines = [ln for ln in msg.splitlines() if not ln.startswith("#")]
    while lines and lines[-1] == "":
        lines.pop()
    while lines and lines[0] == "":
        lines.pop(0)

    if not lines:
        return ["空提交信息，已拒绝"]

    subject = lines[0]
    if subject.startswith(EXEMPT_PREFIX):
        return []

    problems: list[str] = []
    m = SUBJECT_RE.match(subject)
    if not m:
        problems.append(
            f"主题行格式不符：`{subject}`；应为 `<类型>(<可选范围>): <主题>`，"
            f"类型仅限 {'/'.join(sorted(TYPES))}，冒号后必须带空格"
        )
        return problems

    subj = m.group(3)
    if subj.endswith("."):
        problems.append("主题结尾不能加句号")
    if subj and subj[0].isascii() and subj[0].isupper():
        problems.append("主题英文首字母不要大写")
    full_w = display_width(subject)
    if full_w > MAX_LINE:
        over = full_w - MAX_LINE
        problems.append(
            f"主题行显示宽度 {full_w} 列，超过 {MAX_LINE} 列（超 {over} 列；"
            f"中文等全角字符按 2 列计，含 `类型(范围): ` 前缀）"
        )

    rest = lines[1:]
    if rest:
        if rest[0] != "":
            problems.append("主题与正文之间需空行分隔")
        body_lines = 0
        for i, ln in enumerate(rest[1:], start=2):
            w = display_width(ln)
            if w > MAX_LINE:
                problems.append(f"正文第 {i} 行显示宽度 {w} 列，超过 {MAX_LINE} 列（中文等全角字符按 2 列计）")
            if ln != "" and not is_footer_block(rest[1:], i - 2):
                body_lines += 1
        if body_lines > MAX_BODY_LINES:
            problems.append(
                f"正文有效行数 {body_lines} 行，超过 {MAX_BODY_LINES} 行；"
                f"请考虑拆分提交，或将根因/论证沉淀至 docs/ 或 cairn/ 后附指针"
            )
        for ln in rest:
            if ln.startswith("BREAKING CHANGE:") or ln.startswith("BREAKING-CHANGE:"):
                if not ln.split(":", 1)[1].strip():
                    problems.append("BREAKING CHANGE 需带描述")
            elif ln.startswith("Closes"):
                if not CLOSES_RE.match(ln):
                    problems.append(f"Closes 格式不符：`{ln}`；应为 `Closes #123, #456`")

    return problems


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8")
    if len(sys.argv) < 2:
        print("用法：check-commit-msg.py <提交信息文件>")
        return 2
    msg = Path(sys.argv[1]).read_text(encoding="utf-8", errors="replace")
    problems = check(msg)
    if not problems:
        print("提交信息格式校验通过")
        return 0
    for p in problems:
        print(f"格式错误：{p}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
