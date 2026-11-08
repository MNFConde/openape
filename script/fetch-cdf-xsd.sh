#!/bin/sh
# 取 ASAM CDF 2.0.0 规范 XSD 到 tmp/（不入库），按固定 sha256 校验后安装。
#
# 为什么不钉直链：ASAM 官网现行 2.1.0 的下载走会员登录，历史 2.0.0 页同样标注
# 会员，免登录只给四个 PDF；公开可得的 XSD 在第三方仓库（christoph2/asamint，
# GPL-2）里，直链会随对方目录结构漂移，钉死等于埋一条到期即坏的下载路。
# 故本脚本只锚定校验和：给 --from 或 --url 都行，哈希不符一律拒装。
#
# 用法：
#   script/fetch-cdf-xsd.sh --from <xsd 或 zip>   # 装已有件（zip 里抽同名文件）
#   script/fetch-cdf-xsd.sh --url  <下载地址>     # 先下再校验
#   script/fetch-cdf-xsd.sh                       # 只打印获取指引与期望哈希
#   可选 --out <路径>（默认 tmp/cdf/cdf_v2.0.0.sl.xsd）
#
# 消费方：format-cdf 的实现依据（元素集常量表即从此件整理）。代码与测试不读它，
# 所以它缺席不影响构建与门禁。
set -eu

NAME=cdf_v2.0.0.sl.xsd
SHA256=715f16e4146621a973aef7cf6dd968a16d112d431efb25e412727bc922920169
ASAM_PAGE=https://www.asam.net/standards/detail/cdf/

die() {
	printf '错误：%s\n' "$*" >&2
	exit 1
}

usage() {
	cat <<'EOF'
用法：
  script/fetch-cdf-xsd.sh --from <xsd 或 zip>   装已有件（zip 里抽同名文件）
  script/fetch-cdf-xsd.sh --url  <下载地址>     先下再校验
  script/fetch-cdf-xsd.sh                       只打印获取指引与期望哈希
  可选 --out <路径>（默认 tmp/cdf/cdf_v2.0.0.sl.xsd）
EOF
}

sha256_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d' ' -f1
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | cut -d' ' -f1
	else
		die "缺少 sha256 工具（需要 sha256sum 或 shasum）"
	fi
}

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT="$ROOT/tmp/cdf/$NAME"
SRC=
URL=
WORK="$ROOT/tmp/cdf/.work"

cleanup() {
	rm -rf "$WORK"
}
trap cleanup EXIT

while [ $# -gt 0 ]; do
	case $1 in
	--from)
		[ $# -ge 2 ] || die "--from 需要一个路径"
		SRC=$2
		shift 2
		;;
	--url)
		[ $# -ge 2 ] || die "--url 需要一个地址"
		URL=$2
		shift 2
		;;
	--out)
		[ $# -ge 2 ] || die "--out 需要一个路径"
		OUT=$2
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	*) die "未知参数：$1（用 --help 看用法）" ;;
	esac
done

if [ -n "$SRC" ] && [ -n "$URL" ]; then
	die "--from 与 --url 只能给一个"
fi

if [ -z "$SRC" ] && [ -z "$URL" ]; then
	cat <<EOF
未给来源，不做任何事。获取指引：

  1. 打开 ASAM CDF 标准页：$ASAM_PAGE
  2. 2.0.0 需会员登录；拿到规范包后取出 $NAME
     （公开第三方来源为非官方镜像，自行判断其可信度）
  3. 装到本仓：script/fetch-cdf-xsd.sh --from <下载路径>
     或 direct 下载：script/fetch-cdf-xsd.sh --url <地址>

期望 sha256：$SHA256
安装位置（不入 git）：tmp/cdf/$NAME

哈希不符会被拒——本脚本的意义就是别再靠"看起来是那份文件"。
EOF
	exit 1
fi

verify() {
	actual=$(sha256_of "$1")
	if [ "$actual" != "$SHA256" ]; then
		die "sha256 不符，拒绝安装
  文件：$1
  期望：$SHA256
  实际：$actual"
	fi
}

is_zip() {
	# 内容优先于后缀：--url 下来的临时件没有 .zip 后缀
	[ "$(head -c 2 "$1" 2>/dev/null)" = "PK" ] && return 0
	case $1 in *.zip | *.ZIP) return 0 ;; esac
	return 1
}

extract_zip() {
	rm -rf "$WORK"
	mkdir -p "$WORK"
	if command -v unzip >/dev/null 2>&1; then
		unzip -q -o "$1" -d "$WORK" || die "解包失败：$1"
	elif command -v python3 >/dev/null 2>&1; then
		python3 -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])' \
			"$1" "$WORK" || die "解包失败：$1"
	else
		die "解包 $1 需要 unzip 或 python3"
	fi
	found=$(find "$WORK" -type f -name "$NAME" | head -n 1)
	[ -n "$found" ] || die "压缩包里没有 $NAME：$1"
	printf '%s\n' "$found"
}

install_from() {
	from=$1
	[ -f "$from" ] || die "找不到文件：$from"
	if is_zip "$from"; then
		candidate=$(extract_zip "$from")
	else
		candidate=$from
	fi
	verify "$candidate"
	mkdir -p "$(dirname -- "$OUT")"
	cp "$candidate" "$OUT"
	printf '已安装：%s\nsha256：%s\n该件不入 git（tmp/ 已忽略）。\n' "$OUT" "$SHA256"
}

if [ -n "$URL" ]; then
	command -v curl >/dev/null 2>&1 || die "下载需要 curl"
	mkdir -p "$ROOT/tmp/cdf"
	raw="$ROOT/tmp/cdf/.download"
	curl -fsSL "$URL" -o "$raw" || die "下载失败：$URL"
	install_from "$raw"
	rm -f "$raw"
else
	install_from "$SRC"
fi
