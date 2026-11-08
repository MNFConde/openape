#!/bin/sh
# 从上游重新生成 vendor/a2ltool-dwarf（裁剪副本，只留 DWARF/PDB 提取层）。
#
# 上游：https://github.com/DanielT/a2ltool（MIT OR Apache-2.0），默认 tag v3.4.2。
# 只复制三处：src/debuginfo/、src/symbol.rs、两份 LICENSE；src/lib.rs 是本仓新写
# 的薄导出层，不覆盖。取哪些、为何这么裁见 vendor/a2ltool-dwarf/README.md。
#
# 用法：
#   script/vendor-a2ltool.sh              # 用默认 tag
#   script/vendor-a2ltool.sh v3.5.0       # 换 tag（先在 tmp/ 里备好该 tag 的克隆）
#   A2LTOOL_REPO=<url> script/vendor-a2ltool.sh   # 换上游地址（镜像/私有 fork）
#
# 步骤：tmp/ 里没有克隆就先 clone 该 tag → 复制三处 → 打印 git diff 摘要。
# 复制后 git diff **只应看到可见性改动**与两处 mod test 删除（详见 crate README）；
# 出现别的差异说明上游逻辑变了，要逐条看，别直接提交。
set -eu

TAG=${1:-${A2LTOOL_TAG:-v3.4.2}}
URL=${A2LTOOL_REPO:-https://github.com/DanielT/a2ltool.git}

die() {
	printf '错误：%s\n' "$*" >&2
	exit 1
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
DST="$ROOT/vendor/a2ltool-dwarf"
SRC="$ROOT/tmp/vendor/a2ltool"

[ -d "$DST" ] || die "找不到 $DST——本脚本只更新已有副本，不凭空造 crate"
command -v git >/dev/null 2>&1 || die "需要 git"

if [ ! -d "$SRC/.git" ]; then
	printf '在 %s 克隆 %s（tag %s）…\n' "$SRC" "$URL" "$TAG"
	mkdir -p "$(dirname -- "$SRC")"
	git clone --quiet --depth 1 --branch "$TAG" "$URL" "$SRC" ||
		die "克隆失败：$URL（tag $TAG 是否存在？git ls-remote --tags $URL）"
fi

cur=$(git -C "$SRC" describe --tags --always 2>/dev/null || echo unknown)
if [ "$cur" != "$TAG" ]; then
	printf '提示：%s 当前在 %s，不是 %s。换 tag 需先删掉该目录再重跑：\n  rm -rf %s\n' \
		"$SRC" "$cur" "$TAG" "$SRC"
fi

for f in src/debuginfo src/symbol.rs LICENSE-MIT LICENSE-APACHE; do
	[ -e "$SRC/$f" ] || die "上游缺 $f——目录结构变了，先看上游再改本脚本"
done

rm -rf "$DST/src/debuginfo"
mkdir -p "$DST/src"
cp -R "$SRC/src/debuginfo" "$DST/src/debuginfo"
cp "$SRC/src/symbol.rs" "$DST/src/symbol.rs"
cp "$SRC/LICENSE-MIT" "$SRC/LICENSE-APACHE" "$DST/"

printf '\n已从 %s（%s）复制到 %s\n\n' "$SRC" "$cur" "$DST"
printf '接下来核对差异（应只有可见性改动 + 两处 mod test 删除）：\n'
printf '  git -C %s diff --stat -- vendor/a2ltool-dwarf\n' "$ROOT"
printf '  git -C %s diff -- vendor/a2ltool-dwarf | grep -E "^[-+][^-+]" | head -40\n' "$ROOT"
