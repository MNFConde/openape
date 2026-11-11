# AGENTS.md

OpenAPE —— Rust 开发的开放汽车标定工具：headless daemon 加桌面（Tauri，仅 Windows）与网页双前端，XCP-on-CAN 采集与标定写入，ELF / A2L / DBC 解析，前端代码只有一套；远程访问与认证由前置网关承担，见 [13-deployment](docs/13-deployment.md)。当前状态：**M4 已完成（17/17），M5 推进中（15/33 项完成；2026-11-10 边界自查新增 M5-28～M5-33 六项：后端抽象的三处摩擦与五处权责灰区，见 todo）**——M4 的写入面（`xcp-proto` 短写与 `GET_SEED/UNLOCK`、`xcp` 写通路、`sim-ecu` 分页与解锁、`session` 写前读回/快照/审计/回滚与条件触发、`proto`/`daemon`/`cli`/`ui` 写入与页对比、`bus-zlg` 装配）与 CDF 导入（[D-24](docs/decisions.md#d-24-数据集对接格式改判-cdf)）均已落地，**已知缺口**：CDF 按 2.0.0 实现（2.1.0 差异未验证、轴引用未支持、真机抽验落 M5-08）、`bus-zlg` Windows FFI 侧未验。M5 已完成：M5-01 数据源接口（曲线只认 `SampleSource`）、M5-02/03 MF4 写出端与常时录制（mf4-rs wrapper + 背压记账恒等式）、M5-04 触发窗口统一走录制器出口（时间轴逐行可比）、M5-05 MF4 读取 + 离线重开（`OpenRecording` RPC，离线与实时同一条渲染路径）、M5-06/07 quirk 扩展点与 sim-ecu 怪癖注入（TOML 填 trait，vcan 复现填充与回显差异）、M5-10 回放后端收尾（ReplayBus 盖日志时间戳 [D-25](docs/decisions.md#d-25-回放样本的时间戳语义)、daemon `--replay` 接线，回放与实时同一条采集通路）、M5-11 socketcand 客户端（`bus-socketcand` crate + daemon `--socketcand`，真守护进程 + vcan 端到端通；报文无换行按括号分帧等实测坑见 crate 文档与 todo）、M5-15 契约 crate examples + semver-checks 门（第七条门禁）、M5-16 cli doctor、M5-17 性能面板 + `openape metrics`、M5-14 GitHub Actions 门禁（`.github/workflows/ci.yml` 七个 job，push main 与 PR 都跑；vcan 缺失直接失败，关键 vcan 测试再加哨兵断言防静默跳过）。M5-12 已完成（DTO 头按 D-27 落地）：UDP 传输（`UdpXcpBus` 无头/CTR 头双模式）+ daemon `--xcp-udp` + sim-ecu UDP 端点，回环全链通；**XCPlite 互通达成面已验 + DTO 数据流已通到总线层**（CONNECT/事件发现/动态 DAQ 配置全序列逐条被参考实现接受，DTO 每 1ms 一拍且计数递增，`script/xcplite-interop.sh`），**DTO 头已按 D-27 落地**（描述式档案 + `--dto-header` + 相对号映射 + `rejected` 计数与一次性告警），**daemon 端到端复验通过**（XCPlite 从站 + `--xcp-udp --xcp-udp-headered --dto-header xcplite --daq-event 1 --address-extension 0x01 --daq-timestamp`：采集约 930 fps、丢帧 0）（私有头 `[相对 ODT][0xAA][DAQ list u16][时间戳 32]` 由档案承接，理由见 D-27）；CONNECT 响应已改标准布局（原为自研私有口径），workspace 版本随之 bump 0.2.0。**未完成**：M5-08 真机复验、M5-13 桌面壳（`crates/ui-tauri` 已落地并实测：窗口导航到 daemon 自己发的页面、sidecar 起停与崩溃重起正确；**不勾**是因为 Windows 侧真 daemon 还编不出来，见 M5-20）、M5-18 mutator 台账、M5-19 性能哨兵与基准基线（三层：不变量哨兵进 cargo test、criterion 微基准、CI 回归门）、M5-20 Windows 侧 daemon 可编可跑（socketcan 按平台门控 + `bus-zlg` 修到能编，M5-13 的真缺口）、M5-24 CAN 信号并入观测量；**工作台线（M5-21 语义模型 → M5-22 运行时改采集 → M5-23 演示环境 → M5-25 主界面 / M5-26 标定悬浮窗，另 M5-27 驱动者归属与写权接管）按 [D-28](docs/decisions.md#d-28-运行时配置面与-guidaemon-权责边界) 草案推进**——现有界面只是渲染通路验证，不是交付形态。M5-09 完成：CAN-FD 能力位按接口 MTU 判（与内核发送路径同判据），FD 链路一个 socket 收两种帧形态，64 字节 FD 双向与 8 字节经典形态在 vcan 上验，真机能力位抽验落 M5-08。此前 **M0 风险 spike 与前置设计全部完成**，**分层架构已定稿**（[11-architecture-layers](docs/11-architecture-layers.md)），进度以 todo.todo 为准；cairn 沉淀层已初始化（独立 git 仓库，不入本仓库）。
待办编号在 M0–M5 之外新增 `RH-*`（不挂里程碑、不带里程碑头的勾）：RH-01（rust 三 skill 收口的 7 条）已完成 6/7，前 5 项已合入 main，第 1+2 条（采集泵暂停握手 + 收工走命令通道）在 [PR #2](https://github.com/MNFConde/openape/pull/2) 待合；RH-02 是等契约层 breaking 窗口的暂缓项（含 `bus` 公开面取舍的记录要求），RH-03～RH-06 是 2026-11-10 架构复核出的四条代码侧改动（写闸门收进 `session`、proto 按域拆 service、前端视图层、观测量 ID 空间归属），四项均见 todo。同批已把装配归责、两把锁命名、CLI 形态与 XCP 两条接缝的分工写进 [11-architecture-layers](docs/11-architecture-layers.md) 与 `.archify/` 候选，四份 HTML 已重生成并通过 validate/deliver/check/browser-check。
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
