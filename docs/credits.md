---
type: reference
status: active
summary: "外部致谢与参考登记：依赖、行为参考、格式与规范三类来源，注明许可证与借鉴内容"
updated: 2026-11-07
---

# 外部致谢与参考

三类来源分开登记：**依赖**（编译进产物）、**行为参考**（读其实现理解协议或算法，代码自写）、**格式与规范**（标准文档）。前两类都要在此留出处——不引入上游代码不等于可以不留引用。

登记要求：每条注明许可证与**具体借鉴了什么**；实质借鉴的文件在文件头加来源与许可证注释；许可证以本文件与 `Cargo.lock` 为准，许可证合规检查由 `cargo-deny` 承担。

## 依赖

按 [00-overview.md](00-overview.md#复用与自研边界) 的复用清单落地，逐项许可证在依赖引入时核实。需要特别留意的是**文件级 copyleft** 类（MPL 系）：作为库链接不传染，但对上游代码的本地修改有独立义务，涉及 fork 时须单独确认。

| 项目 | 用途 | 许可证 |
|---|---|---|
| socketcan-rs | SocketCAN 异步访问、FD、vcan | 待引入时核实 |
| a2lfile | A2L 读写 | 待引入时核实（MPL 系，注意文件级义务） |
| roxmltree | 只读 XML 树（CDF 解析） | MIT（已核实） |
| can-dbc | DBC 解析 | 待引入时核实 |
| mf4-rs | MDF4 读写 | 待引入时核实 |
| blf_asc | BLF 与 ASC 日志回放 | 待引入时核实 |
| tonic 与 prost | gRPC 与 grpc-web、消息编解码 | 待引入时核实 |
| tracing 及生态 | 结构化日志 | 待引入时核实 |
| uPlot | Web 曲线渲染 | 待引入时核实 |

## 行为参考

读了实现以理解协议或算法，产物代码为本项目自写。这是本项目"参照而非依赖"的主要形态（见 [decisions.md](decisions.md) D-16）。

| 项目 | 借鉴内容 | 许可证 | 落地位置 |
|---|---|---|---|
| automotive | XCP master 的命令序列、DAQ 配置流程、页切换处理顺序；不引入其代码 | MIT | `xcp` crate，自研实现 |
| a2ltool | DWARF 遍历与类型树展开方式（fork 后提公开性复用，见 D-02） | 待核实 | `format-elf` |
| xcp-lite 与 XCPlite | XCP 从站命令处理、地址粒度约定（D-12 裁决不引入） | Apache-2.0 OR MIT | `sim-ecu`，自研实现 |
| pyXCP | 测试期第三方 master 对照，验证从站行为 | 待核实 | 仅测试，不进产物依赖 |

## 格式与规范

| 规范 | 用途 |
|---|---|
| ASAM MCD-1 XCP | 协议命令帧、DAQ 与页切换语义 |
| ASAM MCD-2 MC（A2L） | 标定描述文件结构与关键字 |
| ASAM CDF | 标定数据集交换格式（XML + schema）。取代原计划的 MCD-1 DCM——后者已不在 ASAM 现行目录，见 [D-24](decisions.md#d-24-数据集对接格式改判-cdf)。**按 2.0.0 实现**：官网现行 2.1.0 的下载只对 ASAM 会员开放，公开可得的是 2.0.0 的 XSD（其头部声明版权归 ASAM），故 schema **不随仓入库**，"越出 schema 即拒绝"由 `format-cdf` 自带的元素集校验承担 |
| ASAM MDF4 | 录制文件格式 |
| ISO 11898（CAN 与 CAN-FD） | 帧格式与 CAN-FD 位速率切换 |
| DWARF 标准 | 调试信息结构 |
| DBC（Vector 事实标准） | 报文与信号描述 |
