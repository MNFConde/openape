---
type: adr
status: accepted
summary: "OpenAPE 决策记录（D-xx 单文件形态）"
updated: 2026-11-05
---

# 决策记录

单文件形态（约 10 条以内）；超过 10 条拆 `adr/` 一篇一文件。需求"做什么"见 [requirements.md](requirements.md)，本文只记"为什么这么选"。

> 现状（2026-11-05）：已 18 条，超阈值。拆分需迁移全部 `#d-xx` 锚点与外部引用，风险高于收益，故并入 M1 文档工作一并处理，此处登记为技术债，不静默破例。

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
- 验证结论（2026-11-05，M0-4 spike）：**ELF 前端不自研 DWARF 解析，复用 a2ltool 的 `debuginfo` 逻辑**。上游该模块全为 `pub(crate)`（lib 目标存在但只导出 `fuzz`），fork 后把 `debuginfo`/`symbol` 提为 `pub`、模块内 `pub(crate)` 改 `pub` 即可当库用——4 文件 52 行、纯可见性调整、无逻辑变更。实测能取到名/地址/大小、数组维度与 stride、结构体成员偏移、位域、枚举值与成员自动展开，同名 static 以 `is_unique=false` 标出。A2L 侧同样可行：`a2lfile` 走"模板 → 改 → 写 → 读回"，`AXIS_DESCR COM_AXIS` + `AXIS_PTS_REF` 的轴归属写读回完整保留，新增对象用 clone 模板条目 + `set_name` 即可（构造函数多为 `pub(crate)`）。注意三点：迭代器给叶子级（需自行分层顶层与成员）、`-O2` 折叠掉的变量静默缺失、无 `.debug_info` 时的报错文案可直接复用为加载体检提示。细节见 `tmp/spike-m0-4/REPORT.md`。

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
- 更正说明（2026-11-05）：本决策的"基于 automotive 二次封装"已被 [D-16](#d-16-xcp-master-自研) 推翻——`xcp` 改为自研 master，automotive 降为**行为参考**，不再作为依赖。D-07 保留原判断以留决策轨迹。

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
- 更正说明（2026-11-05）：裁决事实（`CanAdapter` 可外挂）仍成立，但**采用路径作废**——[D-16](#d-16-xcp-master-自研) 决定 `xcp` 自研后不再存在"把自研后端挂到 automotive 上"的需求。ZLG 与 Vector 等后端改在 [D-03](#d-03-平台与硬件) 的 `Bus` 抽象下实现，与 XCP 层解耦。

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

## D-16 XCP master 自研

- 状态：已决策
- 日期：2026-11-05
- 规范位置：[11-architecture-layers.md](11-architecture-layers.md#三道屏蔽墙)、[credits.md](credits.md#行为参考)
- 背景：[D-07](#d-07-xcp-master-复用) 决策"基于 automotive 二次封装"、[D-11](#d-11-automotive-后端外挂裁决) 裁决"其 adapter 可外挂自研后端"。但 automotive 把 XCP 会话逻辑（DAQ 配置、页切换、上下载）与 CAN 传输实现焊在同一个类型上：一旦需要以太网等其它传输，其上层逻辑无法复用；同时 IP 直接依赖会让单人维护的上游接口变动持续穿透到本项目。
- 决策：`xcp` crate **自研 XCP master**；automotive 降为**行为参考**——阅读其命令序列与处理顺序，代码自写，在 [credits.md](credits.md) 登记并致谢。D-07 与 D-11 的"复用 automotive"部分随之失效。协议帧编解码抽为 L1 的 `xcp-proto`，由 master（`xcp`）与从站（`sim-ecu`）共享。
- 依据：（1）M0-2 已自研从站协议层，命令码、帧对齐填充、ODT 编号等细节已掌握；（2）主从共享编解码，从结构上消除两侧协议理解漂移；（3）自主实现让 CAN 与以太网在同一抽象下平等，符合 [D-01](#d-01-协议与范围) 留传输层的口径；（4）D-11 的实测已证明其上层逻辑不可分离，所谓薄封装名不副实。
- 影响：`xcp` 增加 DAQ 配置逻辑的自研工作量（评估 2–3k 行，M2 内消化）；automotive 退出依赖表；ZLG 与 Vector 后端统一走 `Bus` 抽象，D-11 的"外挂 automotive 后端"路径作废；协议行为另以 pyXCP 与 ASAM 标准对照。

## D-17 crate 分档与依赖方向

- 状态：已决策
- 日期：2026-11-05
- 规范位置：[11-architecture-layers.md](11-architecture-layers.md#crate-分档与依赖方向)
- 背景：外部差异（硬件、传输介质、信息源格式）会在多个 crate 同时出现；若只靠 crate 边界而不定依赖方向，半年后必然出现"上层判别后端名"和环形依赖。
- 决策：crate 分六档——L0 `diag` / L1 契约（`cal-core`、`xcp-proto`、`proto`）/ L2 实现（`bus`、`bus-*`、`format-*`、`xcp`）/ L3 编排（`session`、`sim-ecu`）/ L4 装配（`assembly`）/ L5 外壳（`daemon`、`cli`、`ui`）。依赖只准向下、同档不互赖；跨 crate 类型只在 L1 定义；跨边界一律对象安全 trait。**三道墙**为 `Bus`、`XcpConnection`、`CalibrationSource`，另加 `BusCapabilities` 供上层按能力降级——禁止按后端名分支。只有装配层可指名具体后端 crate。
- 依据：（1）"加 Vector 只改装配一行"这一诉求只能靠依赖倒置满足，trait 本身不够；（2）`diag` 被全层依赖，必须自身零业务依赖才不成环；（3）类型单一源与统一错误转换是接口一致性的最低成本手段；（4）方向靠门禁而非记忆，故 `script/check-deps.py` 按分档表校验 Cargo.toml（M1 落地）。
- 影响：新增硬件 = 新建 `bus-*` crate 加装配层一个分支；`xcp` 与 `session` 不动；`cargo semver-checks` 与 `cargo-deny` 一并入门禁；`00-overview.md` 拓扑表按本分档重排并登记新 crate。

## D-18 双前端共享引擎

- 状态：已决策
- 日期：2026-11-05
- 规范位置：[11-architecture-layers.md](11-architecture-layers.md#双前端共享引擎)
- 背景：项目需要两种形态——常驻作业（daemon 加 UI，本地或远程）与单次探测（一条命令看现象，供脚本与 AI 代理编排快速测试环境）。若各自实现，行为必然分叉。
- 决策：编排逻辑（设备装配、DAQ 环形缓冲、录制、状态机、设备锁）落在 L3 的 `session` 纯库；`daemon` 与 `cli` 是其两个外壳，前者常驻、后者进程即退。CLI 遵守四条纪律：一条命令即一次自足事务、分阶段可组合（`probe bus` / `probe xcp` / `probe source` / `probe ecu`）、输出双模（人类可读与 `--json`，结构源同 `proto`，退出码按故障类别分段）、超时内建。设备独占由 `session` 按（后端、通道）加进程级锁，另有 `--via-daemon` 经本地 gRPC 复用正在运行的会话。
- 依据：（1）一套实现天然保证两种形态行为一致；（2）结构化输出让外部脚本与 AI 代理不必解析散文，探测循环可自动化；（3）CAN 硬件通常不允许两进程同开，锁与转发路径必须前置，否则 CLI 与 daemon 会互相打死。
- 影响：`daemon` 退化为薄外壳（`session` 加 gRPC 与静态托管）；`proto` 必须同时服务 UI 与 CLI 输出；`cli` 的探测子命令与 `doctor` 自检共用判定实现。

## D-19 可观测层合并

- 状态：已决策
- 日期：2026-11-05
- 规范位置：[11-architecture-layers.md](11-architecture-layers.md#可观测层)
- 背景：维测日志、故障上报、性能监测三项都是横切关注点，共用会话与 ECU 上下文；分散成多个 crate 会产生多份初始化、多次依赖声明与不一致的上下文字段。
- 决策：三者合并为 L0 的 `diag` crate（内部 `log` / `fault` / `metrics` 三模块）。日志分三层：常规运行日志、维测帧级记录（运行时开关）、黑匣子环形缓冲（故障触发时落盘）。故障用单一 `Fault` 结构（稳定编号、严重度、上下文、可操作建议、错误链），编号按域分段并与 CLI 退出码对齐；`FaultClassifier` 是**唯一**允许判别后端差异的位置；上报走 `FaultSink` 多实现，外部上报默认关闭。一等指标五项：DAQ 帧率、丢帧数、命令延迟分位、背压排队深度、进程内存。
- 依据：（1）共用上下文与单次初始化即可覆盖三件事；（2）黑匣子针对现场偶发问题，事后追加日志开关无效；（3）错误归一化集中在唯一位置，才能让上层全用 `?` 而不写后端判别；（4）丢帧是高频采集最先崩的地方，必须是一等指标。
- 影响：所有 crate 依赖 `diag`，其零业务依赖成为硬约束（见 [D-17](#d-17-crate-分档与依赖方向)）；UI 侧只做一个统一故障面板；`diag` 本身是 M1 首批落地内容。

## D-20 公开面分层承诺

- 状态：已决策
- 日期：2026-11-05
- 规范位置：[11-architecture-layers.md](11-architecture-layers.md#公开面策略)
- 背景：既要让别人能基于本项目继续开发（不能因抽象不合理或函数不公开而用不起来），又不能把所有函数都公开——后者等于给内部实现永久承诺 semver。
- 决策：按层承诺——L1 契约全公开且 semver 严格；L2 实现只公开 trait 实现与少量构造器，内部模块 `pub(crate)`；L3 编排对外一个 facade 加配置与事件枚举、字段全私有；L5 外壳不对外。四条机制：facade 手势（私有字段、构造走 `new`/Builder）、扩展点必须在 trait 且可 `dyn` 取出、半成品放 `pub mod unstable` 并声明不保证兼容、每个契约 crate 至少一个只用公开 API 的 `examples/` 流程作为抽象验收。反向规则：不为"将来可能有用"而公开，先 `pub(crate)`，放开是兼容变更、收紧是破坏性变更。
- 依据：（1）example 写不出来即抽象不足，这是最廉价也最真实的可用性检验；（2）扩展点落在 trait 上而非函数上，外部才能在不改本项目的前提下接入新硬件与新信息源；（3）私有字段让内部结构调整不构成破坏性变更。
- 影响：`examples/` 成为契约 crate 的完成标准之一；公开面变更由 `cargo semver-checks` 把守（见 [D-17](#d-17-crate-分档与依赖方向)）。

