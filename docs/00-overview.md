---
type: architecture
status: active
summary: "OpenAPE 总纲：定位、workspace 拓扑规划、复用与自研边界"
updated: 2026-11-07
---

# 总纲

OpenAPE 是一个 Rust 开发的**开放汽车标定工具**（对标 CANape/INCA 的开源替代路线）：headless daemon + Web UI 双形态，XCP-on-CAN 完成观测量采集与标定量读写，ELF / A2L / DBC 解析，本地网页与远端网页同一套 UI。需求与边界见 [requirements.md](requirements.md)，选型依据见 [decisions.md](decisions.md)——本篇只放结构与拓扑，不复述它们。

## workspace 拓扑规划

> 本表为规划稿，随实现落地；新增 crate 必须登记进本表。档位与依赖方向规则见 [11-architecture-layers.md](11-architecture-layers.md#crate-分档与依赖方向)，本篇只列职责。

| crate（规划名） | 档 | 职责 | 主要复用 |
|---|---|---|---|
| `diag` | L0 | 维测日志、指标、故障码与分类统一出口 | tracing、自研 Fault |
| `cal-core` | L1 | 内部 IR：变量树、观测量/标定量模型、Curve/Map/轴、转换规则、dataset 模型、quirk/trait 扩展点 | — |
| `xcp-proto` | L1 | XCP 帧编解码：CTO/DTO、DAQ、页切换；master 与从站共享 | 自研，参考 automotive |
| `proto` | L1 | gRPC 契约与消息类型；同时是 CLI 结构化输出 schema 的源 | tonic、prost |
| `format-elf` | L2 | ELF+DWARF → IR（维度取自 DWARF，轴归属归工程配置） | object、gimli，vendor 自 a2ltool 的 debuginfo（见 `vendor/a2ltool-dwarf/README.md`） |
| `format-a2l` | L2 | A2L → IR；IR → A2L 生成 | a2lfile |
| `format-dbc` | L2 | DBC → 报文/信号模型；信号 pack/unpack（自研，与 can-dbc 逐条对照） | 自研 |
| `format-dcm` | L2 | DCM 标定数据集只读导入 | 自研解析 |
| `format-mf4` | L2 | 测量数据录制/导出 | mf4-rs、CSV |
| `bus` | L2 | async Bus trait 与 `BusCapabilities`：帧类型位（经典/FD）、硬件时间戳、通道能力 | — |
| `bus-socketcan` / `bus-socketcand` / `bus-zlg` / `bus-vector` / `bus-replay` | L2 | 各后端 | socketcan-rs 4.0、自研 socketcand 客户端、ZLG 官方库 FFI、Vector XL API（后补）、blf_asc |
| `xcp` | L2 | XCP master：会话、DAQ 配置、页切换、上下载；内部经 `XcpConnection` 隔离传输介质 | 自研，automotive 仅作行为参考 |
| `sim-ecu` | L3 | XCP 从站模拟器：CAN 传输层 + 虚拟内存 + 怪癖注入配置 + vcan | 自研，见 [D-12](decisions.md#d-12-xcp-on-can-从站自研) |
| `session` | L3 | 编排引擎：设备装配、DAQ 环形缓冲、录制、状态机、设备锁 | — |
| `assembly` | L4 | 唯一指名具体后端 crate 的装配层：A2L 与 ELF 的交叉校验、描述文件装配（`load_variables`）、按配置选总线后端（`backend::open`） | — |
| `daemon` | L5 | 常驻外壳：`session` + gRPC 服务 + 静态托管 | tonic（grpc-web） |
| `cli` | L5 | 单次探测外壳：一条命令一次事务，输出人类可读或 JSON | — |
| `ui` | L5 | Web 前端：曲线（uPlot）、标定量表格、dataset 对比、故障面板 | TypeScript + uPlot |

依赖版本只钉在 workspace 根 `Cargo.toml` 一处。服务端与前端构建链以 M0-3 spike 实测通过的一组为基线（该组同时满足 grpc-web 流式与单端口托管）：`tonic` 0.14 + `tonic-prost` 0.14 + `tonic-web` 0.14 + `prost` 0.14、`axum` 0.8、`tower` 0.5、`tower-http` 0.6；服务端代码生成走 `tonic-prost-build`——0.14 起 prost 集成已从 `tonic-build` 拆出，用旧路径生成不出期望的 trait。升级这组依赖前必须重跑 grpc-web 流式回归。

## 仓库顶层结构

| 目录 | 职责 | git 归属 |
|---|---|---|
| `docs/` | 治理文档（地图见 [README.md](README.md)） | tracked |
| `fixtures/` | 入库测试夹具：ELF 溯源 C 源码、A2L golden（见 [fixtures/README.md](../fixtures/README.md)） | tracked |
| `.agents/rules/` | 低频协作规则，条件加载 | tracked |
| `script/` | 门禁检查脚本与台账 | tracked |
| `.githooks/` | commit-msg 钩子薄壳 | tracked |
| `cairn/` | Project Cairn 沉淀层：LOG、RULES、知识专题文档 | 独立 git 仓库，本项目不入踪 |
| `.cairn/` | Cairn 机器配置（冻结配置与 spec date） | 不入踪 |

## 复用与自研边界

> 调研基准日 2026-11-04；维护状态与许可证详情见调研报告存档，此处只留结论。

- **直接复用**：socketcan-rs 4.0（含 FD/异步/vcan）、a2lfile / a2ltool（A2L 读写与 DWARF→A2L）、object / gimli（ELF/DWARF）、can-dbc（DBC 解析）、mf4-rs（MDF4 读写）、blf_asc（BLF/ASC）、tonic 与 prost（gRPC/grpc-web）、tracing（结构化日志）、uPlot（Web 曲线）
- **必须自研**：XCP master 与会话层（automotive 仅作行为参考，见 [D-16](decisions.md#d-16-xcp-master-自研)）、XCP 帧编解码、XCP-on-CAN 从站（见 [D-12](decisions.md#d-12-xcp-on-can-从站自研)）、DBC 信号 pack/unpack 语义（字节序/因子/多路复用）、socketcand 协议客户端、ZLG zlgcan FFI 后端、DCM 解析器、quirk 外部配置层、Curve/Map 轴点注记语法、故障分类学与可观测层
- **参考而非依赖**：阅读上游实现以理解协议或算法，产物代码自写，出处统一登记在 [credits.md](credits.md)
- 依赖均为单人或实验性维护的项目，统一隔离在 trait 边界内，必要时 vendored fork（见 [decisions.md](decisions.md) D-07）；外部差异的定量屏蔽点见 [11-architecture-layers.md](11-architecture-layers.md#三道屏蔽墙)

## 本域索引

| 文档 | 职责 | 何时读 |
|---|---|---|
| [README.md](README.md) | 文档地图与稳定度分组 | 找文档时 |
| [requirements.md](requirements.md) | 需求基线与验收口径 | 扩范围/验收前 |
| [11-architecture-layers.md](11-architecture-layers.md) | 分层架构：三道屏蔽墙、crate 分档与依赖方向、双前端共享引擎、可观测层、公开面承诺 | 新建 crate / 定接口 / 接新硬件前 |
| [10-axis-binding-rfc.md](10-axis-binding-rfc.md) | Curve/Map 轴点注记约定与绑定配置格式 | 实现变量模型或 ELF→A2L 导出时 |
| [decisions.md](decisions.md) | 决策记录 D-xx | 查"为什么这么选"时 |
| [credits.md](credits.md) | 外部依赖、行为参考与规范的三类出处登记 | 引入依赖或借鉴上游实现时 |
| [AGENTS.md](AGENTS.md) | docs/ 写作约束 | 写/改 docs/ 前 |
