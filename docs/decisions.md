---
type: adr
status: accepted
summary: "OpenAPE 决策记录（D-xx 单文件形态）"
updated: 2026-11-08
---

# 决策记录

单文件形态（约 10 条以内）；超过 10 条拆 `adr/` 一篇一文件。需求"做什么"见 [requirements.md](requirements.md)，本文只记"为什么这么选"。

> 现状（2026-11-08）：已 27 条，超阈值。拆分需迁移全部 `#d-xx` 锚点与外部引用，风险高于收益，故并入 M1 文档工作一并处理，此处登记为技术债，不静默破例。

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
- 更正说明（2026-11-05，M3 开工前）：**自定义 section 标记与用户显示属性一并撤销**，ELF 档只做基本功能，完整语义必须由 A2L 提供。理由：D-13 已把角色判定交给用户、把语义层收归 A2L 档，section 标记失去承载对象，只剩「来源注记」用途，用 `MOD_PAR` 的 `COMMENT` 即可，不必发明新关键字（自定义 block 在 `a2lfile` 严格模式下要么被拒要么静默丢失）；用户显示属性是被否掉的旁车 YAML 的显示子集，维护成本同 A2L 却想省掉 A2L，且与 A2L 的 `COMPU_METHOD` 形成双源。随之澄清第三项：**DWARF 只给维度（事实），轴归属是猜测**——`float cal_map[4][3]` 的 4×3 可信，哪维用哪个符号当轴不可信，故轴归属一律由工程配置给出（10 号 RFC）。
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
- 更正（2026-11-05）："native GUI 不做"已逆转，见 [D-21](#d-21-native-gui-决策逆转)；"daemon 同时监听 localhost 与 LAN"改为默认仅回环，见 [D-22](#d-22-远程接入由前置网关承担)。

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
- 更正说明（2026-11-07）：**对外互通格式由 DCM 改判为 CDF**，见 [D-24](#d-24-数据集对接格式改判-cdf)。"内部用 JSON + 只读导入 + schema 带版本字段"三条不变；改的只是"读哪种外来文件"。D-08 原文保留以留决策轨迹。

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

- 复核结论（2026-11-05，M2-15）：前述"两处非标准行为"的表述**不成立**，二者都是标准行为，项目实现无需改动。
  - **GET_CAL_PAGE 响应三字节**：正响应载荷是 `reserved_1`、`reserved_2`、`logical_data_page_number` 各一字节。依据是两份互不相关的实现——scapy 的 `CalPagePositiveResponse(reserved_1, reserved_2, logical_data_page_number)`（`scapy.contrib.automotive.xcp.cto_commands_slave`），以及 pyXCP 的 `getCalPage` 取 `response[2]` 配其测试帧 `FF 00 00 55`。sim-ecu 现返回 `[0x00, 0x00, page]`，位置与之一致。
  - **WRITE_DAQ 后条目指针自增**：XCP 规定写条目后 ODT 条目指针自增，master 连续写同一 ODT 的多个条目时只发一次 `SET_DAQ_PTR`。pyXCP 的 `writeDaq` 每次调用只发一条 `WRITE_DAQ`（命令码 `0xE1` + 7 字节参数），不自带重设指针，正是依赖这条自增；我们的 master 同样只在每个 ODT 起始处发一次指针命令。
  - 口径说明：ASAM 标准正文为付费文档，未能取得条款原文，因此结论建立在两份独立开源实现与自研从站的互通实测上，而非标准引用。若日后取得条款原文，应回填到本段。

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
- 依据：（1）"加 Vector 只改装配一行"这一诉求只能靠依赖倒置满足，trait 本身不够；（2）`diag` 被全层依赖，必须自身零业务依赖才不成环；（3）类型单一源与统一错误转换是接口一致性的最低成本手段；（4）**依赖方向由 Cargo 依赖图强制**——crate 只能 `use` 自己声明过的依赖，反向引用在编译期即失败，无需检查脚本或评审纪律；真正会错的是切分本身，故把切分判据与后置项写成文档规则（[11-architecture-layers.md](11-architecture-layers.md#切分判据与后置项)）。
- 影响：新增硬件 = 新建 `bus-*` crate 并在装配处加一个分支，`xcp` 与 `session` 不动；`00-overview.md` 拓扑表按本分档重排并登记新 crate；M1 只建九 crate，`proto` 与独立 `assembly` 按触发条件后置。

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

## D-21 native GUI 决策逆转

- 状态：已决策
- 日期：2026-11-05
- 规范位置：[requirements.md](requirements.md) 需求 6、[13-deployment.md](13-deployment.md)
- 背景：[D-04](#d-04-ui-单套) 以"一套代码覆盖本地与远程"为由排除 native GUI。但此类工具（Canape、INCA、CANoe）的主流形态是本地桌面应用，工程师习惯双击启动；纯浏览器形态在单机场景要求用户先开终端起 daemon，不符合使用习惯。
- 决策：做 Tauri 桌面应用，**仅 Windows 打包**；界面复用 Web UI 的同一份产物，不写第二套 UI；daemon 作为 sidecar 由应用拉起；其它平台仍走浏览器。
- 依据：（1）Tauri 的界面就是网页，前端零改动，代价只是壳与打包链；（2）Windows 的 WebView2 是三平台中最成熟的，Linux 的 WebKitGTK 才是真坑，只做 Windows 正好绕开；（3）GPUI 一类原生框架要重写界面并放弃 web 图表生态，收益集中在"上万行表格 + 高频刷新"这类本阶段不存在的场景。
- 影响：形态不再对称——Windows 有两个入口（桌面、网页），其它平台只有网页；新增打包、签名与 sidecar 生命周期工作，不并入 M2；MVP 完成定义改为"桌面或 Web UI 均可"。

## D-22 远程接入由前置网关承担

- 状态：已决策
- 日期：2026-11-05
- 规范位置：[13-deployment.md](13-deployment.md)
- 背景：[D-04](#d-04-ui-单套) 曾设想 daemon 同时监听 localhost 与 LAN，一步覆盖本地与远程。但本工具能写入 ECU 标定量，LAN 上任何机器连上即可改参数；而认证、多用户、TLS 属于成熟基础设施，自行实现是重造轮子。
- 决策：daemon 默认只绑 `127.0.0.1`，靠 `--listen` 显式开放；daemon 内不做认证、不做 TLS，只承诺 [13-deployment.md](13-deployment.md#daemon-对外契约) 的四条对外契约；远程接入、认证、方法级 ACL 全部由前置网关（nginx / Caddy / 自研）承担；未部署网关即无远程能力。
- 依据：（1）能力按部署组合、代码零分叉，M2 因此不需要任何认证代码；（2）gRPC 方法名就在 URL 路径里，方法级 ACL 用网关的路径规则即可，无需在 daemon 预留权限接口；（3）安全模型一句话说得清——能连上 daemon 即有全部权限，所以永不直接暴露。
- 影响：D-04 的"同时监听 localhost 与 LAN"作废；WSL2 等转发场景不再需要按源 IP 判断"是否本地"；自研精简网关的触发条件见 [13-deployment.md](13-deployment.md#自研精简网关的触发条件)。

## D-23 真机抽验排期

- 状态：已决策
- 日期：2026-11-07
- 规范位置：[requirements.md](requirements.md) 验收口径、[11-architecture-layers.md](11-architecture-layers.md#后置项)
- 背景：档三把真机抽验定为里程碑合入前的闸门（M1 起建清单），其前置条件写的是周立功 CAN-FD 卡加目标 ECU；而 `bus-zlg` 原排在 M5。两者直接冲突——M4 是 MVP 最后一站，没有 Windows 侧后端，档三永远绿不了，而 MVP 完成定义又要求档三已执行。
- 决策：`bus-zlg` 提前到 M4 末尾（`todo.todo` 的 M4-16），验收分两层：**代码面**（能力位如实、帧路径与 socketcan 行为对等、装配按配置选后端，不需要硬件即验）与**真机面**（12 号清单 H-01–H-17 加写入类检查点，记录表填实）。硬件或驱动不在场时，M4 可勾但记录表整段标「未验」，真机复验落 M5-08，不阻塞 MVP。
- 依据：（1）MVP 边界本就把周立功 CAN-FD 卡算在内（见 requirements 的"预留不做"段），把它排到 M5 是排期与边界的错位，不是取舍；（2）`Bus` trait 已把硬件差异关在 L2，装配层加一个配置分支即可，`xcp` 与 `session` 不动，改动面可控；（3）ZLG 的 FFI 面只有开设备、发、收三个（[D-11](#d-11-automotive-后端外挂裁决) 已核实），风险集中在 Windows 构建链与驱动，不在协议；（4）把"硬件是否在场"与"代码是否完成"拆成两层验收，避免把 MVP 完成绑在不可控的外部条件上。
- 影响：M4 需补 Windows 侧构建链与 `zlgcan` 库路径的环境记录（[90-env.md](90-env.md)）；`bus-zlg` 是全仓第一个必须放开 `unsafe` 的位置——现有 15 个 crate 一律 `#![forbid(unsafe_code)]`，该项只在 FFI 模块内局部放开并注明理由；M5 增加一项真机复验（M5-08）。
## D-24 数据集对接格式改判 CDF

- 状态：已决策
- 日期：2026-11-07
- 规范位置：[requirements.md](requirements.md) 能力清单与验收口径、[11-architecture-layers.md](11-architecture-layers.md#切分判据与后置项)、[credits.md](credits.md#格式与规范)
- 背景：[D-08](#d-08-数据集格式) 把 DCM 只读导入定为 MVP 的对外互通入口。M4 动工时这条路走不通：ASAM MCD-1 DCM 已不在现行标准目录（`asam.net/standards/detail/mcd-1-dcm/` 返回 404，现行页只有 MCD-1 CCP / MCD-1 XCP / MCD-2 MC / CDF / MCD-2 CERP），规范正文只剩退役文档渠道；公开样本与现成解析实现同样找不到。按记忆写解析器等于发明格式，与"未经确认的判断不写成既成事实"冲突——M4-06/M4-07 因此停在未动工状态。
- 决策：MVP 的 dataset 对外互通格式由 DCM 改为 **ASAM CDF**（当前 2.x，Base Standard，XML + schema）；crate 名随之改为 `format-cdf`，范围仍是只读导入，内部 dataset 仍是 JSON（[D-08](#d-08-数据集格式) 的其余三条不变）。M4-06/M4-07 按 CDF 重写，仍未落地。
- 依据：（1）CDF 是在册、在维护的 ASAM 标准，规范与配套文档可从官网取得，不像退役的 DCM 那样只剩购买渠道；（2）CDF 是 XML + XSD，能接上档一（权威校验器）与档二（golden 字节基线），DCM 的文本格式给不了这个条件；（3）CDF 覆盖标量、字符串、数组、曲线、map 与结构体，并带单位与质量/版次元信息，正对上我们的标定量模型；（4）工具链支持真实存在（MathWorks 与主流 MC 工具链都读写 CDF），属"事实可用"而非纸面标准。
- 影响：`format-dcm` 不在本期建 crate；requirements 的"MVP 完成定义"与档二 golden 口径一并改为 CDF；[credits.md](credits.md#格式与规范) 的规范表随之替换。DCM 只保留"拿到真实样本后再议"的可能，不写进任何里程碑。**未验证项**：CDF 规范正文与 XSD 的下载方式与许可尚未实测（本会话只确认标准在册与其公开定位），动工前需先取到规范与 schema，并入库一份真实 `.cdf` 样本作 fixtures。

- 复核结论（2026-11-07，M4-06 动工前实测，取上段"未验证项"）：
  - **下载方式**：官网现行版本 2.1.0 的下载按钮走登录弹窗、标注 "Free of charge for members"，需 ASAM 账号；免登录只给 Release Presentation、两份 TOC 与 LOKI 四个 PDF。按常见命名探 `/fileadmin/Standards/CDF/` 的 zip 均 404；历史版本页的 2.0.0 同样标注会员。
  - **XSD 可得性**：公开可得的是 **CDF 2.0.0** 的 XSD/DTD（第三方仓库 `christoph2/asamint` 内，与 pyXCP 同作者），文件头写 "Copyright 2006 ASAM e.V / All rights reserved"。
  - **许可结论**：XSD 属 ASAM 版权物，不随本仓入库（本项目 MIT OR Apache-2.0），也不拷入 fixtures；"越出 schema 即拒绝"改由 `format-cdf` 自带的元素集校验承担（只校验元素、属性一律忽略）。样本不入第三方数据，改为自造并入库（`fixtures/cdf/minimal.cdf`，golden 三件套见 `fixtures/README.md`）。
  - **口径落定**：M4-06 按 2.0.0 实现；2.0.0 与 2.1.0 的差异**未验证**。日后取得 2.1.0 规范正文与其 XSD，再按条款补差并回填本段。
  - **按真实 `.cdfx` 复核（2026-11-07，M4 收尾后）**：拿一份公开的 `.cdfx`（`asamint` 仓库的 CDF20demo，仓库为 GPL-2，**未入库**，只作本地诊断）跑现有解析，四处会挡：① 文件编码是 ISO-8859-1，按 UTF-8 读整份失败；② 取值放在 `SW-INSTANCE-PROPS-VARIANTS/SW-INSTANCE-PROPS-VARIANT/SW-VALUE-CONT`，只认直属 `SW-VALUE-CONT` 时读出 0 条；③ `<VG><LABEL>` 被自家元素集误拒（白名单含 `LABEL`、匹配分支漏了）；④ `CATEGORY` 写成 `DEPEDNDENT_VALUE` 之类的笔误。处理口径随之细化：**元素结构仍严格**（越出即拒），但 ① 编码做 UTF-8→ISO-8859-1 回退、② 变体取值纳入解析（多变体只取第一个并明说）、④ 类别越界改为"留空 + 警告点名原文"而非拒整份文件——拒掉会让用户拿不到其余几百条取值。`SW-AXIS-CONTS` 的**轴引用未实现**（记录在 todo 的未排期项）。样本未入库的原因不变：许可不干净，回归用自造样本（`fixtures/cdf/`）。

## D-25 回放样本的时间戳语义

- 状态：已决策
- 日期：2026-11-08
- 规范位置：[requirements.md](requirements.md)（回放属 M5-10 离线重开面）
- 背景：M5-10 落地回放解析与 `ReplayBus` 后，daemon 接线要把日志里的 DTO 帧喂进 `DaqStream`。`DaqSample::timestamp_us` 现由采集泵按墙钟打（相对采集起点的单调值），实时曲线、录制 MF4、触发窗口共用这一语义；回放喂入时"保留日志时间戳"还是"重打墙钟"未定，卡住了接线。
- 决策：回放喂入**保留日志中录制的时间戳**——时间戳取最接近真实的那个值。`DaqStream` 的喂入侧放开"自带时间戳"入口，同时禁止混源：同一条流里不允许墙钟样本与日志时间样本并存；离线重开走独立会话/流。
- 依据：回放的意义是把真实发生过的数据再看一遍，时间轴是它的一部分——重打墙钟会把离线曲线的时间轴变成"回放发生的那几分钟"，与 MF4/CSV 及触发窗口（M5-04 刚立的时间轴逐行可比判据）全部对不上；保留日志值则四份文件（MF4、CSV、触发窗口、曲线）共享同一条时间轴。
- 影响：`DaqStream` 增加"喂入侧自带时间戳"的入口与混源拒绝；触发窗口/录制逻辑不变（它们只消费时间戳值，不关心来源）；daemon 的 `--replay` 接线按本决策实现。


## D-26 CDF 规范 XSD 随夹具入库

- 状态：已决策
- 日期：2026-11-08
- 规范位置：[../fixtures/README.md](../fixtures/README.md)
- 背景：D-24 判断"ASAM CDF 2.0.0 XSD 版权属 ASAM、不能进本仓"，故 fixtures/cdf 只放手写样本。实现 CDF 解析需要对照规范 XSD 的元素集，长期放 tmp（不入库）会让实现依据不可追溯。
- 决策：`fixtures/cdf/cdf_v2.0.0.sl.xsd` 随夹具入库作实现参考（覆盖 D-24 的原判断）；同批纳入 `fixtures/mf4/ref.mf4`（M5-02/03 期外部参考样本，交叉验证用）。
- 依据：XSD 系 ASAM 公开提供，随仓携带仅作实现参考与测试输入；本仓首次对外分发前须复核 ASAM 条款对再分发的限制，必要时改为下载脚本 + 校验和。
- 影响：fixtures README 的 cdf 段已加更正说明；对外开源发布前须对 ASAM XSD 做一次许可复核（风险已登记于此）。
- 更正说明（2026-11-08，建公开仓库前查历史时执行复核）：**本决策改判，XSD 不入库**。复核结论——ASAM 官网 CDF 页现行 2.1.0 与历史 2.0.0 都标注会员下载，免登录只有四个 PDF；公开可得的 2.0.0 XSD 出自第三方仓库（`christoph2/asamint`，GPL-2）的副本，文件头 "All rights reserved" 没有任何再分发授权措辞。本仓许可为 MIT OR Apache-2.0，把它放进公开仓库等于替 ASAM 做再分发判断，与"未经确认的判断不写成既成事实"冲突。故按本决策「依据」段预留的备选方案执行：**下载脚本 + 校验和**，脚本 `script/fetch-cdf-xsd.sh` 装到 `tmp/cdf/`（不入 git），sha256 锚定 `715f16e4…920169`；不钉直链的原因写在该脚本头部（官方无公开直链、第三方副本地址会漂移）。代码与测试不读这份 XSD，元素集校验仍在 `format-cdf` 内，故取件缺席不影响构建与门禁。
- 同批处置（2026-11-08）：`fixtures/mf4/ref.mf4` 一并删除——它被本决策记为"外部参考样本"属误记，实际是 M5-02 摸底时用 asammdf 8.8.32 现写的（`tmp/mf4-probe/ref.mf4` 字节完全相同、组注释 `openape probe`），且全仓无任何代码或测试引用它，属死夹具。两件都只在同一条提交内进过历史，未推送任何远程，故以改写该提交的方式处置，历史中不留痕。

## D-27 对端私有 DTO 布局的适配层次：标准 > 描述式配置 > 脚本

- 状态：已决策（适配层次定稿；profile 落点见「遗留待定」）
- 日期：2026-11-08
- 规范位置：[../crates/xcp/src/daq.rs](../crates/xcp/src/daq.rs)（`DaqLayout` / `DaqListSpec`）、[../crates/session/src/capture.rs](../crates/session/src/capture.rs)（`pump` 的 DTO 解码）
- 背景：M5-12 与 XCPlite（vectorgrp 参考实现）互通时发现其 DTO 头是私有布局 `[相对 ODT][0xAA][DAQ list u16 LE][32 位时间戳 LE][数据]`，而生产解码（`capture` 的 `split_first`）假定标准 `[PID][数据]`、首字节即绝对 ODT 号。同一族「载荷之外的元数据形状」假设还有三处：`DaqListSpec.timestamp` 只设 `SET_DAQ_LIST_MODE` 的位、回来的从站时间戳从不解析（sim-ecu 未实现该位，故回环测试永远不可见）；地址扩展 0 是标定段相对还是绝对随 ECU 定；CRO 是否补齐到 8 字节靠对端宽容。每接一台新从站都可能各私有一次，故需要定层次而非逐个打补丁。
- 决策：
  1. **默认口径是标准 `[PID][数据]`**——不写任何配置也成立，这是与规范对得上的唯一形态。
  2. **对端差异用描述式配置承接**：头字段表（跳过 N 字节 / 取 `pid` / 取 `daq list` / 取 `timestamp`）、PID 语义（绝对或 list 内相对）、时间戳有无与位宽。配置放**对端 profile**，不从属于单条 DAQ list——这是从站的属性，不是 list 的属性。出向的对话参数（地址扩展、CRO 补齐）同放该 profile。
  3. **脚本不作为实现路径**。仅在描述式表达不了时（位压缩载荷、跨帧状态）再开，且须先把脚本 API 定成受管公开面、配 golden 回归。
- 依据：字节排布可以数据化，语义种类不能——「这段字节是什么」必须先被代码认识，故解释器要有一小撮固定原语（`skip` / `pid` / `list` / `timestamp`），布局靠它们拼。描述式在原生代码里切分，无跨界开销、不引入新依赖与安全面。脚本只覆盖**解码类**，覆盖不了**对话类**（地址扩展要改出向 CRO、甚至先探测段信息）与**传输类**（socketcand 分帧、CRO 补齐）；且逻辑一旦外置就是一条新公开面契约，要 semver 与 golden 回归同管。逐帧调用的跨界成本（参数编组 + 栈帧）编译不掉，只有 Rust cdylib 与 WASM AOT 能把边界压到纳秒级；解释器方案是性价比最差的一档。
- 影响：`DaqLayout`（或对端 profile）新增头布局字段属公开面变化，按 0.x 走 minor；XCPlite 互通从「测试内剥头」升为「生产可配」；**从站时间戳进 `DaqSample::timestamp_us` 是另一个决策**（D-25 禁混源：日志时间、采集起点墙钟，第三源要不要引入本文不代答）；将来若开脚本口，优先 Rust cdylib（信任作者、零跨界）或 WASM AOT（不信任、强沙箱），不选解释器。
- 遗留待定（不阻塞本决策）：profile 落点是 daemon flag（如 `--dto-header`）还是 TOML 文件；是否与 [D-06](#d-06-模拟器与怪癖机制) 的 quirk 配置合并成同一份对端档案。

