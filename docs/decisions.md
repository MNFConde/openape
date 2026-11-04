---
type: adr
status: accepted
summary: "OpenAPE 决策记录（D-xx 单文件形态）"
updated: 2026-11-04
---

# 决策记录

单文件形态（约 10 条以内）；超过 10 条拆 `adr/` 一篇一文件。需求"做什么"见 [requirements.md](requirements.md)，本文只记"为什么这么选"。

## D-01 协议与范围

- 状态：已决策
- 日期：2026-11-04
- 规范位置：[requirements.md](requirements.md)
- 背景：标定协议可选 XCP on CAN / CCP / XCP on Ethernet / 私有协议；标定量存在只读与可写两档。
- 决策：XCP-on-CAN 先行，传输层留 trait，Ethernet 后补；范围 = DAQ 采集 + 标定写入 + 页切换，flash 刷写不进任何近期里程碑；CCP 不支持。
- 依据：XCP 是 ASAM 现行标准、生态主流；以太网硬件门槛高会拖慢落地；刷写工作量数倍于采集标定且风险高。
- 影响：`xcp` crate 以 CAN 传输为主形态；BUS trait 必须先行于协议层。

## D-02 信息源三能力

- 状态：已决策
- 日期：2026-11-04
- 规范位置：[00-overview.md](00-overview.md)
- 背景：变量信息获取方式有运行时直读 ELF、生成 A2L 统一通道、旁车清单三种路线。
- 决策：三能力全要——ELF+DWARF 前端、A2L 前端、ELF→A2L 导出；标记约定 = 自定义 section + DWARF struct 轴关联 + 旁车 YAML（单位/量程等展示属性）。
- 依据：统一 IR 后 A2L 逻辑全复用且可导出给 CANape/INCA 互操作；section+DWARF+旁车是商业 ELF 转 A2L 工具的成熟路数；纯命名前缀表达不了 Curve/Map 轴点与单位。
- 影响：`format-elf` 需设计 section 名与旁车 schema；轴点注记语法是最大设计风险，开工前出 RFC 与 golden 样例（todo M0-5）。

## D-03 平台与硬件

- 状态：已决策
- 日期：2026-11-04
- 规范位置：[00-overview.md](00-overview.md)
- 背景：CAN 硬件生态 Linux SocketCAN 零成本，Windows 商业驱动闭源收费；用户手头目标硬件为周立功 CAN-FD 卡。
- 决策：async Bus trait 统一抽象；起步后端 = socketcan-rs 4.0、socketcand 客户端（自研）、BLF/ASC 日志回放、ZLG 官方库 FFI（Windows/Linux 双端）；Vector/PEAK 后补。
- 依据：零商业依赖起步对开源项目 CI 与贡献者友好；ZLG 是实际在手硬件，FFI 成本可控。
- 影响：`bus` trait 从第一天带 FD 帧类型位与硬件时间戳；ZLG 后端 MVP 内交付（M5 实作）。

## D-04 UI 单套

- 状态：已决策
- 日期：2026-11-04
- 规范位置：[requirements.md](requirements.md)
- 背景：本地 GUI、本地网页、远程网页三种形态组合多。
- 决策：只做 headless daemon + Web UI：daemon 同时监听 localhost 与 LAN，本地打开即本地页，换 IP 即远程页；native GUI 不做。
- 依据：一套代码天然覆盖本地/远程双需求，避免双客户端维护。
- 影响：UI 技术栈为 TypeScript + uPlot，daemon 静态托管前端产物。

## D-05 API 形态

- 状态：已决策
- 日期：2026-11-04
- 规范位置：[00-overview.md](00-overview.md)
- 背景：REST+WebSocket 与 gRPC+grpc-web 两案，权衡在调试门槛与强类型契约。
- 决策：gRPC（tonic）+ grpc-web 先行，proto 即契约；若流式调试难度不可接受，降级为 REST + WebSocket（DAQ 流走 msgpack/二进制帧）。
- 依据：强类型 IDL 对前后端一致性与未来多语言自动化控制端价值大；grpc-web 流式存在已知坑，降级预案前置到 M0-3 spike 验证。
- 影响：M0-3 spike 是 go/no-go 闸门；降级触发即改本文标记并同步 decisions。

## D-06 模拟器与怪癖机制

- 状态：已决策
- 日期：2026-11-04
- 规范位置：[requirements.md](requirements.md)
- 背景：开发期无真 ECU 依赖不可自动化；现成开源中没有完整的"多怪癖 ECU 模拟器"；ECU 怪癖种类繁多。
- 决策：`sim-ecu` = xcp-lite 内核（Vector 官方，Apache/MIT）+ 虚拟内存 + vcan + 怪癖注入配置；本项目怪癖兼容统一走外部声明式配置（TOML/JSON）填 trait 扩展点；诊断等未来协议同样走 trait 插件位，动态插件（WASM）后置。
- 依据：xcp-lite 是现成最专业的 slave 实现；配置式兼容满足"改配置不改代码"的开闭要求。
- 影响：sim-ecu 是 CI 全链路测试的依赖（M1 落地）；`cal-core` 扩展点设计优先级提高。

## D-07 XCP master 复用

- 状态：已决策
- 日期：2026-11-04
- 规范位置：[00-overview.md](00-overview.md#复用与自研边界)
- 背景：XCP 无独立成熟 Rust crate；automotive（MIT，活跃）已实现 XCP master 与多 CAN 后端。
- 决策：`xcp` crate 基于 automotive 二次封装；其 adapter trait 若不能挂自研 ZLG 后端则 fork 抽层（M0-1 spike 裁决）。
- 依据：省掉最磨人的 DAQ 配置/页切换协议逻辑；协议行为另以 pyXCP 与 ASAM 标准对照。
- 影响：上游为单人维护，接口面收在 `xcp` crate 内部，不外漏；M0-1 裁决已出——**可外挂、无需 fork**，见 D-11。

## D-08 数据集格式

- 状态：已决策
- 日期：2026-11-04
- 规范位置：[requirements.md](requirements.md)
- 背景：标定 dataset 需要存储、对比、与既有标定资产互通。
- 决策：内部 dataset 文件用 JSON（diff/版本化友好）；MVP 加 DCM 只读导入（解析器自研）；DCM 双向后补。
- 依据：JSON 对 git diff 与工具链最友好；DCM 是 INCA/WinOLS 事实标准，只读导入已可打通存量资产。
- 影响：`format-dcm` 范围锁定为只读；dataset schema 需版本字段。

## D-09 仓库形态

- 状态：已决策
- 日期：2026-11-04
- 规范位置：[00-overview.md](00-overview.md#workspace-拓扑规划)
- 背景：单 crate 与 workspace 之争；许可影响商业集成意愿。
- 决策：Cargo workspace 按 00 总纲拓扑分 crate；Apache-2.0 OR MIT 双许可；README 中英双语。
- 依据：crate 边界即依赖隔离边界（呼应 D-07）；双许可最大化采用面且与全部选定上游兼容。
- 影响：新 crate 必须登记进 00 总纲拓扑表。

## D-10 沉淀层治理

- 状态：已决策
- 日期：2026-11-04
- 规范位置：[../AGENTS.md](../AGENTS.md)
- 背景：接入 project-cairn 沉淀项目知识；其 init 默认 `git_policy=ignore` 的忽略块会把根 AGENTS.md 一并拖出 git，且默认无"沉淀层自建仓库"机制。
- 决策：`cairn/` 与 `.cairn/` 不入本项目 git（忽略块收窄到这两个目录，**不含 AGENTS.md**）；`cairn/` 内部自建独立 git 仓库跟踪知识历史；根 AGENTS.md 保持 tracked，cairn 规则正文下沉 `cairn/RULES.md`（untracked 侧），根文件只留一行条件声明；CLAUDE.md 桩不建。
- 依据：AGENTS.md 共享面协议——它是项目资产，任何 skill 不得将其拖出 git；知识历史与代码历史关注点不同，独立仓库互不干扰。
- 影响：毕业 provider 暂缓对接（首次毕业时配置）；各规则文件坑登记的详情指针从"就地展开"切换为"另存 cairn/ 知识专题文档"。

## D-11 automotive 后端外挂裁决

- 状态：已决策
- 日期：2026-11-04
- 规范位置：[00-overview.md](00-overview.md#复用与自研边界)
- 背景：D-07 留待 M0-1 spike 裁决——automotive 的 adapter 机制能否挂自研 ZLG 后端，不能则 fork 抽层。
- 决策：**无需 fork**。`CanAdapter` 为公开阻塞式 trait（send/recv 必需，其余带默认实现），外部 crate 直接实现；`AsyncCanAdapter::new<T: CanAdapter + Send + 'static>` 泛型收值起 worker 线程，`XCPOnCAN::new(Arc<AsyncCanAdapter>, XCPCanConfig)` 消费，`fd: true` 构造成功（spike 编译运行实测，SPIKE-GREEN）。
- 依据：源码裁决（automotive @ 2026-11-04 master，默认 `features = []` 核心零后端可编译）+ 编译运行实测；ZLG 后端只需在 `bus-zlg` 填三个 FFI 面——`ZCAN_OpenDevice`（构造）、`ZCAN_Transmit`（send）、`ZCAN_Receive`（recv），BRS 由通道时序配置出 `default_brs`。
- 影响：M5 ZLG 后端按外挂实现；CAN 标准帧 ID 上限 0x7FF，超限用 `Identifier::extended`；生产依赖取 crates.io 版本（spike 用源码路径 dep）；automotive 上游变更风险仍收在 `xcp` crate 内部。
