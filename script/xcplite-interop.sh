#!/usr/bin/env bash
# XCPlite 互通验收（M5-12）：构建参考从站、起跑、算 global_counter 的
# 运行时地址，然后跑 #[ignore] 的互通测试。见 script/scripts.md。
set -euo pipefail
cd "$(dirname "$0")/.."

XCPLITE_DIR=${XCPLITE_DIR:-tmp/XCPlite}
if [ ! -d "$XCPLITE_DIR" ]; then
    echo "缺 $XCPLITE_DIR：git clone --depth 1 https://github.com/vectorgrp/XCPlite（MIT）"
    exit 1
fi

cmake -B "$XCPLITE_DIR/build" -S "$XCPLITE_DIR" \
    -DXCPLITE_BUILD_EXAMPLES=ON -DCMAKE_BUILD_TYPE=Debug >/dev/null
cmake --build "$XCPLITE_DIR/build" --target hello_xcp -j4 >/dev/null

# 从站在自己的目录里跑：它生成的 A2L 分片落 cwd，别污染仓库根。
# exec 让 PID 直接是 hello_xcp 进程，收尾 trap 才杀得掉。
(cd "$XCPLITE_DIR" && exec ./build/hello_xcp > /tmp/hello_xcp.log 2>&1) &
PID=$!
trap 'kill "$PID" 2>/dev/null || true' EXIT
sleep 1

# XCPlite 在 Linux 64 位上的绝对寻址是「模块基址 + 32 位偏移」
# （ApplXcpGetBaseAddr 取 PIE 基址），所以 XCP 侧地址就是符号偏移
# （PIE 下等于 nm 的链接地址），不是运行时绝对地址。
SYMOFF=$(nm "$XCPLITE_DIR/build/hello_xcp" | awk '/ [bBdD] global_counter$/{print $1}')
ADDR="0x$SYMOFF"
echo "global_counter 的 XCP 地址（符号偏移）：$ADDR"

XCPLITE_ADDR=127.0.0.1:5555 XCPLITE_COUNTER_ADDR="$ADDR" \
    cargo test -p openape-xcp --test xcplite_interop -- --ignored --nocapture
