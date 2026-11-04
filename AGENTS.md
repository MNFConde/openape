# AGENTS.md

OpenAPE —— Rust 开发的开放汽车标定工具：headless daemon + Web UI 双形态，XCP-on-CAN 采集与标定写入，ELF / A2L / DBC 解析，本地与远程同一套网页。当前状态：治理结构已初始化（docs/ 规则 门禁 钩子 就位），代码未起步；M0-1 / M0-2 风险 spike 已完成（结论见 docs/decisions.md 的 D-11 / D-12），余下 M0-3 ~ M0-5，进度以 todo.todo 为准；cairn 沉淀层已初始化（独立 git 仓库，不入本仓库）。开发环境：WSL2 Ubuntu-24.04（OpenBitFun WSL 工作区），Windows 侧保留 ZLG 真机验证。

仓库存在 cairn/ 时先读其规则文件（cairn/RULES.md），否则跳过；知识/教训沉淀唯一走 cairn，未安装时由各规则文件的「经验与坑」节就地承担。

## 阅读顺序

1. 先读本文件。
2. 目录级专属约束见 `docs/AGENTS.md` 与 `script/AGENTS.md`，读取对应目录下文件时自动生效。
3. 按需阅读 `docs/` 文档（地图 docs/README.md）：需求基线 requirements → 总纲 00-overview → 选型 decisions。

## 文档协作规则

- 改动前判断用户要「讨论/建议」还是「直接改文档」；说「先看看/先评估」时先给分析，别直接重写正式文档。
- 纠正过往判断时追加更正说明，不静默覆盖。
- 未经确认的判断不写成既成事实。

## 协作约定

- 与用户交流一律使用中文
- 所有下载的文件和临时的脚本，都放入项目根目录的 tmp 下，tmp 不入 git 追踪
- 指令若与仓库文档或既有约定不符，先指出冲突点、说明取舍，再执行
- 项目状态发生变化时，同步更新本文件状态段，避免误导后续会话
- 提交格式规范见 `.agents/rules/commit.md`，仅在准备 commit 时读取；提交相关教训须登记于该文件「经验与坑」节
- 门禁命令与脚本触发方式的单一源分别在 `.agents/rules/commit.md` 与 `script/scripts.md`，他处一律指针回指

## 仲裁（skill 冲突裁决）

- 优先级：**项目内实际约定 > 项目级 skill > 全局 skill > 通用规范集**
- Rust 语言规范以 rust-* 系列 skill 为准（rust-best-practices / rust-async-patterns / rust-testing）；与项目约定冲突时按上行优先级裁决
- 知识/教训沉淀唯一走 cairn；各 skill 的沉淀机制不并行使用

## 质量门禁（提交前）

- 门禁命令单一源见 `.agents/rules/commit.md` 门禁流水线节（cargo fmt/clippy/test/audit + 文档三检查 + commit-msg 钩子）
- 依赖版本只钉 workspace 根 Cargo.toml 一处；升级后必须跑全量测试 + `cargo audit`，勿随手升级
