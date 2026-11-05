---
type: adr
status: accepted
summary: "OpenAPE 决策记录（D-xx 单文件形态）"
updated: 2026-11-05
---

# 决策记录

单文件形态（约 10 条以内）；超过 10 条拆 `adr/` 一篇一文件。需求"做什么"见 [requirements.md](requirements.md)，本文只记"为什么这么选"。

> 现状（2026-11-05）：已 13 条，超阈值。拆分需迁移全部 `#d-xx` 锚点与外部引用，风险高于收益，故并入 M1 文档工作一并处理，此处登记为技术债，不静默破例。

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
- 更正说明（2026-11-05）：**旁车 YAML 不再作为语义来源**——单位/量程等展示属性只在带 A2L 时提供，仅 ELF 档一律显示 raw；标记约定只保留自定义 section + DWARF struct 轴关联，用户手动轴值绑定归本软件工程配置。取舍理由与替代口径见 [D-13](#d-13-信息源两档输入与写入安全)。

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
- 验证结论（2026-11-05，M0-3 spike）：**主案成立，不降级**。Chrome headless 实测 tonic + `GrpcWebLayer` 的 server-streaming 20 帧全收、`grpc-status:0`、连跑 3 次一致，客户端 abort 后服务端 task 正确退出。三条实现约束并入 M2 约定：（1）tonic 0.14 用 `GrpcWebLayer` 替代已移除的 `tonic_web::enable()`，服务端须 `accept_http1(true)`；（2）**daemon 必须单端口同时提供静态页面与 grpc-web**——跨端口时 `CorsLayer` 覆盖不到 OPTIONS 预检（tonic 在路由前返回 400），浏览器侧必挂；做法是 axum 包 tonic service，注意请求体 `axum::body::Body → tonic::body::Body`、响应体反向转换；（3）长流靠 `mpsc::Sender::send` 返回 `Err` 判定客户端断开，需留 task 退出路径。细节见 `tmp/spike-m0-3/REPORT.md`。

## D-06 模拟器与怪癖机制

- 状态：已决策
- 日期：2026-11-04
- 规范位置：[requirements.md](requirements.md)
- 背景：开发期无真 ECU 依赖不可自动化；现成开源中没有完整的"多怪癖 ECU 模拟器"；ECU 怪癖种类繁多。
- 决策：`sim-ecu` = xcp-lite 内核（Vector 官方，Apache/MIT）+ 虚拟内存 + vcan + 怪癖注入配置；本项目怪癖兼容统一走外部声明式配置（TOML/JSON）填 trait 扩展点；诊断等未来协议同样走 trait 插件位，动态插件（WASM）后置。
- 依据：xcp-lite 是现成最专业的 slave 实现；配置式兼容满足"改配置不改代码"的开闭要求。
- 影响：sim-ecu 是 CI 全链路测试的依赖（M1 落地）；`cal-core` 扩展点设计优先级提高。
- 更正（2026-11-05，M0-2 spike）：内核假设不成立——xcp-lite 只有 XCP-on-Ethernet 传输层（Rust 侧传输层枚举仅 UDP/TCP，上游 C++ XCPlite 588 个文件同样无 CAN 传输层），不能充当 CAN 从站；sim-ecu 内核改为自研，见 [D-12](#d-12-xcp-on-can-从站自研)。本决策其余部分（虚拟内存、怪癖注入配置、vcan）不受影响。

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

## D-12 XCP-on-CAN 从站自研

- 状态：已决策
- 日期：2026-11-05
- 规范位置：[00-overview.md](00-overview.md#workspace-拓扑规划)
- 背景：M0-2 spike 要在 vcan 上跑通 XCP-on-CAN 最小环，需要一个可控的 CAN 从站；实测 [D-06](#d-06-模拟器与怪癖机制) 假定的 xcp-lite 内核没有 CAN 传输层，不可用。
- 决策：sim-ecu 的 XCP-on-CAN 从站自研——CAN 传输层与会话层自己实现；spike 版以 Python raw socket 跑通协议（`tmp/spike-m0-2/xcp_can_slave.py`），产品化为 Rust 实现；不采用 xcp-lite，也不为 XCPlite 补 CAN 传输层再维护 fork。
- 依据：（1）上游无 CAN 传输层，补层即自研加维护 fork，成本高于自己写；（2）怪癖注入本就需要掌握协议层细节，自研不额外增加知识负担；（3）spike 用第三方 master（pyXCP）对照，CONNECT / 内存读写 / 页切换 / 静态 DAQ 共 13 项断言连跑 3 次全过，最小环已通；（4）CAN 帧对齐填充、DAQ 绝对 ODT 编号、命令码等细节已在 spike 落地，可直接复用。
- 影响：sim-ecu 增加自研传输层工作量（收在 M1 内）；xcp-lite 退出 sim-ecu 依赖，仅留作以太网 XCP 的对照参考（可选）；pyXCP 的两处非标准行为（GET_CAL_PAGE 响应补齐、WRITE_DAQ 指针自增）待与 ASAM 标准复核，M2 实现完整 DAQ 前完成。

## D-13 信息源两档输入与写入安全

- 状态：已决策
- 日期：2026-11-05
- 规范位置：[requirements.md](requirements.md)
- 背景：M0-5 讨论发现，[D-02](#d-02-信息源三能力) 的"三件套注记"若做成"仅 ELF 也全语义"，等于把 A2L 的人工成本搬去旁车 YAML；而实测 ELF/DWARF 能可靠给出变量名、类型、地址、长度、结构布局、enum，却完全没有单位、量程、物理换算、轴归属、标定/观测角色，任何启发式推断都会误判。
- 决策：
  - **两档输入**。仅 ELF：提供名/类型/地址/长度/布局/enum + 用户自选轴值绑定；不提供语义层。ELF + A2L：全语义。
  - **旁车 YAML 不做语义来源**（[D-02](#d-02-信息源三能力) 已追加更正）；仅 ELF 档的语义缺失字段一律显示 raw 并标注 `[raw]`，不以默认值填充。
  - **轴值绑定归本软件工程配置**：只装绑定关系，轴是独立实体、支持 N:1 共享轴；格式带版本字段并容忍未知字段，为将来扩展留位；轴长与值维度不匹配即拒绝绑定。
  - **不做角色判定**：仅 ELF 档不推断标定量/观测量，由用户指定；写入按不安全操作对待——单点写入、写前读回、写后校验、禁批量整表。
  - **构建前提转为加载体检**：ELF 须带 `-g` 且未 strip；不满足时只警告不阻断，逐条给可操作提示；`static` 同名跨编译单元自动加 CU 前缀。
- 依据：（1）DWARF 无角色/单位/轴归属字段，启发式误判的代价是写坏 ECU 内存；（2）XCP `DOWNLOAD` 按地址直写、从站不校验语义，角色不明时必须收紧写路径；（3）旁车的维护成本与 A2L 同级，违背"仅 ELF 更省事"的初衷；（4）轴长度可由 DWARF 或符号大小硬校验，绑定本身可信。
- 影响：M0-5 的 RFC 范围收窄为"轴为独立实体 + 引用 + 绑定 + 工程配置格式"，单位与换算移出（归 A2L 档）；IR 语义字段统一 `Option`；录制文件与 dataset schema 需带模式标记（仅 ELF / 带 A2L），避免两档数值被混比。

