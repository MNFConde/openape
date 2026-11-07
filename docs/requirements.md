---
type: requirements
status: active
summary: "OpenAPE 需求基线：能力清单、MVP 边界、非功能约束、验收口径"
updated: 2026-11-07
---

# 需求基线

本文是需求唯一源：做什么、不做什么、验收口径。选型"为什么"不在本文（见 [decisions.md](decisions.md)），结构"怎么放"不在本文（见 [00-overview.md](00-overview.md)）。

## 能力清单

1. **CAN 信号本地读取**：经典 CAN 与 CAN-FD，DBC 解码出物理信号值，本地硬件接入与日志回放
2. **观测量采集**：XCP-on-CAN DAQ，毫秒级推流到 UI 曲线
3. **标定量读写**：读取/写入 ECU 标定量、页切换；不支持 flash 刷写。仅 ELF 档角色由用户指定，写入须单点、写前读回、写后校验、禁批量整表（[decisions.md](decisions.md) D-13）
4. **变量信息来源**：ELF（仅 ELF，DWARF）与 A2L 双前端建立变量结构；额外提供 ELF → A2L 导出。两档输入口径——仅 ELF 时提供变量名/类型/地址/长度/结构布局与用户自选轴值绑定，不提供单位、量程、物理换算、轴归属与标定-观测角色（角色由用户指定）；带 A2L 时提供全部语义。详见 [decisions.md](decisions.md) D-13
5. **DBC 支持**：报文/信号解析与物理值换算
6. **UI 形态**：Windows 桌面应用（Tauri 壳，与网页共用同一份前端产物）与全平台网页（daemon 托管，本地或远端）；界面代码只有一套，见 [decisions.md](decisions.md#d-21-native-gui-决策逆转)
7. **标定数据集**：JSON dataset、对比 diff、DCM 只读导入
8. **录制导出**：MDF4 与 CSV
9. **两种使用形态**：常驻作业（daemon 加桌面或 Web UI，本地或远程）与单次探测（CLI 一条命令看现象，输出人类可读或 JSON，供外部脚本与 AI 代理编排测试环境）；两者共享同一套编排实现，见 [decisions.md](decisions.md) D-18
10. **可观测性**：维测日志（帧级记录与运行时开关、故障时自动落盘最近窗口）、全系统统一故障上报（稳定编号、可操作建议、UI 统一故障面板）、内部性能指标（含丢帧数），见 [decisions.md](decisions.md) D-19

## 问题定位与标定作业能力

真实标定与"查软件问题"场景下需要、且上述能力清单未显式覆盖的项。落点按里程碑给，避免范围失控。

| 能力 | 说明 | 落点 |
|---|---|---|
| 条件触发录制 | 信号满足条件时自动录制前后缓冲窗口，用于偶发故障复现 | M4，与录制器同批 |
| 写入审计与回滚快照 | 每次写入前自动快照为 dataset，写操作记审计日志（谁、何时、写了什么、写前写后值），失败可回滚 | M4，与标定写入同批，不拆分 |
| 参考页与工作页对比 | XCP 双页的页间 diff 与复制，避免整页误覆盖 | M4，与页切换同批 |
| A2L 与 ELF 交叉校验 | 同一 ECU 的两份文件在地址、尺寸、类型上不一致时出冲突报告，`probe source` 阶段即可发现 | M3 |
| A2L 量程写入前校验 | 带 A2L 时用其 min/max 拦截越界写入 | M4 |
| MF4 离线重新打开分析 | 历史测量文件作为数据源重新加载与对比，与实时流共用同一套 UI 数据源接口 | M5 |

## MVP 边界

MVP 不做的：见下"不做 / 预留不做 / 怪癖兼容"三段。

- **不做**：XCP flash 刷写、UDS/J1939 诊断、CCP 独立支持、WASM 动态插件；多 ECU 时间同步、STIM 注入、多用户协作亦不做——三者都会把 `session` 从单会话改成调度器，与"先减后加"的结构方向相反，MVP 后按真实需求重议
- **已逆转的不做项**：native GUI 改为 Tauri 桌面应用（仅 Windows，其它平台走浏览器），见 [decisions.md](decisions.md#d-21-native-gui-决策逆转)；"多用户协作"仍不做——多人查看同一路采集由前置网关承担，多人同时调度同一 ECU 不在范围内，见 [13-deployment.md](13-deployment.md)
- **预留不做**：诊断协议须能在不动/少动核心代码下加入（trait 插件位，开闭原则）；**DTC 读取（UDS 0x19）**属问题定位场景的硬缺口但 MVP 不实现——M2 定义契约时留扩展位与数据形状，触发条件为"有人用本软件查真实 ECU 故障"；XCP-on-Ethernet 传输后补；Windows 商业驱动（Vector/PEAK）后补——周立功 CAN-FD 卡除外，属 MVP
- **怪癖兼容**：核心功能不内置 ECU 怪癖；通过外部声明式配置（TOML/JSON）+ trait 扩展点尽可能兼容，改配置不改代码

## 非功能约束

- 开源：Apache-2.0 OR MIT 双许可；Cargo workspace 多 crate；README 中英双语
- 复用优先：选型先查活跃开源库（调研基准日 2026-11-04，结论见 [00 总纲复用与自研边界](00-overview.md#复用与自研边界)）；自研只补缺口
- 依赖隔离：单人维护/实验性上游统一收在 trait 边界内
- 可扩展与可复用：外部差异（硬件、XCP 传输介质、信息源格式）只从三个 trait 进入，新增硬件或传输不改内部逻辑；公开面按层承诺，第三方能基于本项目的公开 API 与扩展点继续开发，见 [11-architecture-layers.md](11-architecture-layers.md#公开面策略)
- 平台起步：Linux SocketCAN 系（含 vcan、socketcand、日志回放）；Windows 侧经 ZLG FFI 支持周立功 CAN-FD 卡——`bus-zlg` 提前到 M4 末尾落地，理由与降级规则见 [decisions.md](decisions.md#d-23-真机抽验排期)
- 远程接入边界：daemon 默认仅绑回环，进程内不做认证与 TLS；远程访问、认证与方法级权限由前置网关承担，未部署网关即无远程能力，见 [13-deployment.md](13-deployment.md)

## 验收口径

- **MVP 完成定义**：todo.todo 中 M1–M4 全部勾完——即：vcan 上 sim-ecu 与 daemon 全链路采集/标定写入/页切换跑通；A2L 与 ELF 双前端 + ELF→A2L 导出出件；曲线与标定量表格在 Web UI 本地/远程可用；dataset 对比与 DCM 导入可用；CLI 分阶段探测命令与统一故障面板可用
- **档二 golden（适用）**：ELF→A2L 导出在 M3 落地、DCM 解析在 M4 与实现同批落地——golden 不能先于被测对象存在；fixtures 入库，纯重构以零 diff 为通过线
- **档三 人工抽验（适用，M1 起建清单）**：里程碑合入前用真机抽验——周立功 CAN-FD 卡 + 目标 ECU，检查点矩阵见 [12-hardware-checklist.md](12-hardware-checklist.md)（M1 产出）。2026-11-07 裁决：抽验的软件路径由 `bus-zlg`（Windows）在 M4 末尾提供；硬件或驱动不在场时该里程碑可勾，但抽验记录整段标「未验」，不得声明抽验通过，复验落 M5 的 M5-08，见 [decisions.md](decisions.md#d-23-真机抽验排期)
- 门禁（代码层）见 `.agents/rules/commit.md`，与本文档验收口径（产物层）并存
