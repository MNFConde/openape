#!/bin/bash
# 一键门禁：三门禁检查 + 钩子活性双探针（固化自 M0 迁移期 tmp/m0-probe.sh）。
# 用法：bash script/verify-all.sh（任意目录执行均可，自动定位仓库根）。
set -u
cd "$(dirname "$0")/.." || exit 2

fail=0
check() {
  name=$1
  shift
  python3 "$1" || { echo "FAIL: $name"; fail=1; }
}
echo "=== gates ==="
check check-links script/check-links.py
check check-frontmatter script/check-frontmatter.py
check check-markers script/check-markers.py

echo "=== hook liveness ==="
t=$(mktemp -d) || exit 2
printf 'bad message no type\n' > "$t/bad.txt"
printf 'docs: 活性探针\n' > "$t/good.txt"
if ./.githooks/commit-msg "$t/bad.txt" >"$t/bad.log" 2>&1; then
  echo "FAIL: hook accepted a malformed message"
  fail=1
else
  echo "ok: hook rejected malformed message"
fi
if ./.githooks/commit-msg "$t/good.txt" >"$t/good.log" 2>&1; then
  echo "ok: hook accepted a valid message"
else
  echo "FAIL: hook rejected a valid message"
  cat "$t/good.log"
  fail=1
fi
rm -rf "$t"
exit $fail
