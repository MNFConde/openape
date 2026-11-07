# AGENTS.md

OpenAPE —— Rust 开发的开放汽车标定工具：headless daemon 加桌面（Tauri，仅 Windows）与网页双前端，XCP-on-CAN 采集与标定写入，ELF / A2L / DBC 解析，前端代码只有一套；远程访问与认证由前置网关承担，见 [13-deployment](docs/13-deployment.md)。当前状态：**M3 完成（12/12）**——M2 的 daemon 单端口托管、Web 曲线页、CLI 经 daemon 复用会话之后，M3 补齐了信息源与显示层：`format-elf`（DWARF 抽取、加载体检、跨 CU 前缀、进 IR）、ELF→A2L 导出并过 `a2lfile` 严格模式、`format-dbc`（自研解析 + pack/unpack，与 can-dbc 逐条对照）、`assembly` 装配层做 A2L 与 ELF 交叉校验、`session` 按 DBC 观测总线、CLI 的 `probe source`、界面变量表格与故障面板；档二导出 golden、轴绑定 golden 三件套均已入库。**已知缺口**：DCM 导入随 M4；真机抽验仍在 [12-hardware-checklist](docs/12-hardware-checklist.md)。下一步是 M4。此前 **M0 风险 spike 与前置设计全部完成**，**分层架构已定稿**（[11-architecture-layers](docs/11-architecture-layers.md)），进度以 todo.todo 为准；cairn 沉淀层已初始化（独立 git 仓库，不入本仓库）。
开发环境：WSL2 Ubuntu-24.04（OpenBitFun WSL 工作区），Windows 侧保留 ZLG 真机验证。

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
