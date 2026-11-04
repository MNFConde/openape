# AGENTS.md

script/ 目录约定（本文件只约束 script/ 内的脚本组织与固化；根 AGENTS.md 不复述此处内容，读取本目录文件时自动生效）。

## script/ 目录约定

- 定位：需要**持久化、本地运行**的脚本一律放本目录；脚本保持**零依赖档**——纯标准库、禁引第三方包（环境分档依据见 vibe-project-init `references/scripts.md`；Rust 侧一次性工具放 `xtask`，不入本目录）。
- 组织：单文件 `{name}.py`，本身即入口；复杂脚本才升级为 `{name}/` 包 + 薄入口。
- 统一触发：`python3 script/{name}.py`（细则与各机器前缀见 [scripts.md](scripts.md) 台账——台账是触发方式的单一源）。
- 台账：[scripts.md](scripts.md) 每脚本登记用途与门禁挂点一行，含「临时探针台账」节；新脚本必须登记。

## 三次法则

- 同一功能的分析/验证逻辑**第 3 次出现**必须固化进本目录并在台账登记（功能相同即算重复，参数/目标文件变化不算新脚本；单行 shell 惯用语不算；会话内重写与跨会话重现同等计数）。
- 固化后只准扩展既有脚本（新 CLI 参数或新函数并登记用法），禁止再为该功能另写临时脚本散落。
- 一次性诊断探针（strings/strace/单行试探类不立门槛）在台账「临时探针台账」记一行；跨会话累计 3 次同样升级固化。写探针前先查台账。

## 经验与坑（必选登记）

> 本目录相关的教训必须在此登记一份精简版（坑/对策/校验/详情指针）；详情另存 cairn/ 知识专题文档并附指针，不得只沉淀在会话记忆里。

### 1. 商店 python 占位 stub：四级探测只剩 uv 级

- 坑：本开发机 python/python3 为 Microsoft Store 占位 stub（`command -v` 找得到、一跑就败），py 未装——四级探测前三级全灭。
- 对策：钩子薄壳用 `-c` 实跑判定 + 第 4 级 `uv run --no-project python` 兜底；人工跑检查脚本直接用 uv 前缀。
- 校验：`uv run --no-project python script/check-frontmatter.py` 输出 OK。
- 详情：[.agents/rules/commit.md](../.agents/rules/commit.md) 坑登记第 1 条（2026-11-04 实测）。
