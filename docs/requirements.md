---
type: requirements
status: active
summary: "OpenAPE 需求基线：能力清单、MVP 边界、非功能约束、验收口径"
updated: 2026-11-05
---

# 需求基线

本文是需求唯一源：做什么、不做什么、验收口径。选型"为什么"不在本文（见 [decisions.md](decisions.md)），结构"怎么放"不在本文（见 [00-overview.md](00-overview.md)）。

## 能力清单

1. **CAN 信号本地读取**：经典 CAN 与 CAN-FD，DBC 解码出物理信号值，本地硬件接入与日志回放
2. **观测量采集**：XCP-on-CAN DAQ，毫秒级推流到 UI 曲线
3. **标定量读写**：读取/写入 ECU 标定量、页切换；不支持 flash 刷写。仅 ELF 档角色由用户指定，写入须单点、写前读回、写后校验、禁批量整表（[decisions.md](decisions.md) D-13）
4. **变量信息来源**：ELF（仅 ELF，DWARF）与 A2L 双前端建立变量结构；额外提供 ELF → A2L 导出。两档输入口径——仅 ELF 时提供变量名/类型/地址/长度/结构布局与用户自选轴值绑定，不提供单位、量程、物理换算、轴归属与标定-观测角色（角色由用户指定）；带 A2L 时提供全部语义。详见 [decisions.md](decisions.md) D-13
5. **DBC 支持**：报文/信号解析与物理值换算
6. **UI 双形态**：本地网页（daemon 监听 localhost）与远端网页（同 daemon 换 IP），一套代码
7. **标定数据集**：JSON dataset、对比 diff、DCM 只读导入
8. **录制导出**：MDF4 与 CSV
9. **两种使用形态**：常驻作业（daemon 加 Web UI，本地或远程）与单次探测（CLI 一条命令看现象，输出人类可读或 JSON，供外部脚本与 AI 代理编排测试环境）；两者共享同一套编排实现，见 [decisions.md](decisions.md) D-18
10. **可观测性**：维测日志（帧级记录与运行时开关、故障时自动落盘最近窗口）、全系统统一故障上报（稳定编号、可操作建议、UI 统一故障面板）、内部性能指标（含丢帧数），见 [decisions.md](decisions.md) D-19

## MVP 边界

MVP 不做的：见下"不做 / 预留不做 / 怪癖兼容"三段。

- **不做**：XCP flash 刷写、UDS/J1939 诊断、CCP 独立支持、native GUI、WASM 动态插件
- **预留不做**：诊断协议须能在不动/少动核心代码下加入（trait 插件位，开闭原则）；XCP-on-Ethernet 传输后补；Windows 商业驱动（Vector/PEAK）后补——周立功 CAN-FD 卡除外，属 MVP
- **怪癖兼容**：核心功能不内置 ECU 怪癖；通过外部声明式配置（TOML/JSON）+ trait 扩展点尽可能兼容，改配置不改代码

## 非功能约束

- 开源：Apache-2.0 OR MIT 双许可；Cargo workspace 多 crate；README 中英双语
- 复用优先：选型先查活跃开源库（调研基准日 2026-11-04，结论见 [00 总纲复用与自研边界](00-overview.md#复用与自研边界)）；自研只补缺口
- 依赖隔离：单人维护/实验性上游统一收在 trait 边界内
- 可扩展与可复用：外部差异（硬件、XCP 传输介质、信息源格式）只从三个 trait 进入，新增硬件或传输不改内部逻辑；公开面按层承诺，第三方能基于本项目的公开 API 与扩展点继续开发，见 [11-architecture-layers.md](11-architecture-layers.md#公开面策略)
- 平台起步：Linux SocketCAN 系（含 vcan、socketcand、日志回放）；Windows 侧经 ZLG FFI 支持周立功 CAN-FD 卡

## 验收口径

- **MVP 完成定义**：todo.todo 中 M1–M4 全部勾完——即：vcan 上 sim-ecu 与 daemon 全链路采集/标定写入/页切换跑通；A2L 与 ELF 双前端 + ELF→A2L 导出出件；Web UI 曲线与标定量表格本地/远程可用；dataset 对比与 DCM 导入可用；CLI 分阶段探测命令与统一故障面板可用
- **档二 golden（适用，M3 落地）**：ELF→A2L 导出与 DCM 解析做字节级 golden 测试，fixtures 入库，纯重构以零 diff 为通过线
- **档三 人工抽验（适用，M1 起建清单）**：里程碑合入前用真机抽验——周立功 CAN-FD 卡 + 目标 ECU，检查点矩阵随 M1 产出
- 门禁（代码层）见 `.agents/rules/commit.md`，与本文档验收口径（产物层）并存
