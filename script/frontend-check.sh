#!/usr/bin/env bash
# 前端检查：类型检查 + 生产构建。
#
# 不进提交门禁（口径见 docs/13-deployment.md 与 .agents/rules/commit.md）：
# 耗时长、依赖本地 node 环境，挂 pre-commit 只会逼人用 --no-verify，连带
# 绕过 commit-msg 校验。有 CI 后挂 CI。
set -euo pipefail

cd "$(dirname "$0")/../ui"

if [ ! -d node_modules ]; then
  npm ci
fi

npm run build
