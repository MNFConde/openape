---
type: architecture
status: active
summary: "分层架构：三道屏蔽墙与能力描述、crate 六档依赖方向、双前端共享引擎、可观测层、公开面承诺与代码组织"
updated: 2026-11-07
---

# 分层架构

本文回答"结构怎么放才挡得住外部差异"：抽象边界在哪、crate 之间谁依赖谁、两种使用形态如何共享一套实现、对外承诺多少公开面。crate 的职责清单在 [00-overview.md](00-overview.md#workspace-拓扑规划)，本文不复述职责；职责看总纲、分档与规则看本篇。

## 三道屏蔽墙

外部差异（硬件型号、XCP 传输介质、信息源格式）只允许从三个 trait 进入。编排引擎 `session` 只认这三个 trait 与 `cal-core` 的 IR，不认任何实现 crate 的具体类型。

| 墙 | trait | 屏蔽的差异 | 实现方 |
|---|---|---|---|
| 帧 | `Bus` | CAN 硬件与通道形态：socketcan / socketcand / ZLG / Vector / 日志回放 | `bus-*` 各后端 crate |
| 协议 | `XcpConnection` | XCP 传输介质（CAN、UDP、以太网）与命令族差异 | `xcp` 内部实现 |
| 信息源 | `CalibrationSource` | ELF / A2L / 未来其它描述文件的语义档位差异 | `format-*` 各前端 |

### 能力描述优先于后端判别

`Bus` 必须同时给出 `BusCapabilities`：是否 CAN-FD、时间戳精度、通道数与命名、是否回环、最大发送速率。

上层一律按能力降级，**禁止按后端名分支**（`if backend == "zlg"` 视为架构违规）。新增硬件时只填能力表，上层逻辑不动。这一点比 trait 本身更容易被忽视——trait 只保证"能接上"，能力描述才保证"接上之后上层不用改"。

### 装配层唯一

只有装配层知道具体后端 crate 的存在。`xcp` 不依赖任何 `bus-*`，只依赖 `Bus` trait；具体后端在装配层实例化为 `Arc<dyn Bus>` 注入。独立装配 crate 属后置项，M1 由 `session` 兼任（见下节「切分判据与后置项」）。

```
xcp  ──依赖──> bus（trait）
装配 ──依赖──> bus-socketcan / bus-zlg / bus-vector …  并注入
```

因此新增一种硬件 = 新建一个 `bus-*` crate + 装配层加一个配置分支；`xcp` 与 `session` 的 Cargo.toml 和源码均不动。

## crate 分档与依赖方向

依赖只准向下，同档之间不互相依赖。

| 档 | crate | 约束 |
|---|---|---|
| L0 基础 | `diag` | 不依赖任何业务 crate，否则成环 |
| L1 契约 | `cal-core`（M1 仅基础类型）、`xcp-proto`、`proto`（M1 空档） | 跨 crate 的类型只准在这里定义，且保持轻依赖 |
| L2 实现 | `bus`、`bus-*`、`format-*`、`xcp` | 只准依赖 L0 与 L1 |
| L3 编排 | `session`、`sim-ecu` | 只准依赖 L0 至 L2 |
| L4 装配 | `assembly` | 唯一允许指名具体后端 crate 的位置；当前只做 A2L 与 ELF 交叉校验 |
| L5 外壳 | `daemon`、`cli`、`ui` | 只准依赖 L0 至 L4 |

```
ui ─> proto <─ cli        daemon ─> proto
                  └─> session ─> xcp ─> bus ─> bus-*
                         │       └─> xcp-proto ─> cal-core <─ format-*
                         └────────────────────────────────────┘
     diag ←──────────── 以上全部（含 bus / xcp / format）
```

三条硬约束：

1. `diag` 被所有层依赖，它自己不得依赖任何业务 crate——依赖它最方便，一旦它反过来依赖 `cal-core` 就成环。
2. `bus-*` 只依赖 `bus`，不依赖 `session` 或 `xcp`；装配由 L4 完成。
3. `sim-ecu` 与 `xcp` 共享 `xcp-proto` 的帧编解码，主从两侧的协议理解只有一份。

## 切分判据与后置项

### 依赖方向由构建图强制

crate 只能 `use` 自己 Cargo.toml 里声明过的依赖，反向引用在**编译期**即失败。依赖方向被固化在 Cargo 依赖图里——违规依赖在 Rust 里根本写不出来，不是需要人或脚本去发现的问题。本节的档位表是**这个既成事实的记录与切分理由**，不是执行者；不设依赖方向检查。

真正会做错的是切分本身：分错了照样编译通过，边界白设。判据只有四条，不满足任何一条时同 crate 内模块就够：

| 切 crate 的充分理由 | 说明 |
|---|---|
| 需要物理上不可能反向引用的边界 | 契约类的唯一不可替代理由 |
| 依赖重量差一个量级 | 契约 crate 保持轻依赖，实现 crate 才拖重库——否则下层的消费者被迫背上全家桶 |
| 出现第二个消费者 | 只有一个消费者时抽离没有收益 |
| 用户需要裁剪编译 | 真实存在想 feature 掉某个后端的用户 |

配套的轻依赖约束：L0 与 L1 只许 `serde` / `thiserror` 一类基础库，不得依赖 L2 及以上，也不得引入重实现库。这条是"依赖重量差"判据的延续，同样靠 Cargo.toml 声明守住。

`toml` 归入"基础库"一侧（M2-03 起 `cal-core` 依赖它）：工程配置的格式是需求的既有约定（[10-axis-binding-rfc](10-axis-binding-rfc.md) 第 4 节），换掉它等于换配置格式；它不依赖本项目任何 crate，不会把 L1 拖进重实现链。

**代码生成器不受轻依赖约束**：`prost` / `tonic` 的产物就是契约本身，不是"被重实现的轮子"。判据是生成的类型是否出现在 crate 的公开签名里——是，则它的依赖同样是契约的一部分。这条防止 `proto` 一落地就与 L1 约束字面冲突，同时不放开"L1 不得依赖 L2 及以上"。

### proto 契约约定

`proto` 同时是 CLI `--json` 与 gRPC 的唯一类型源（[00-overview.md](00-overview.md#workspace-拓扑规划)），因此定三条：

1. **同一份类型兼作 serde 载体**。`build.rs` 用 prost-build 的 `type_attribute` 给生成类型挂 `#[derive(Serialize, Deserialize)]` 与 `#[serde(rename_all = "snake_case")]`，字段名保持 snake_case。不引 pbjson（它按 proto3 JSON 映射把字段转 camelCase、int64 转字符串，等于为符合协议改动对外 JSON 形状），也不手写 `impl Serialize`（同一事实两处表达，漂移即产生错误数据）。
2. **数值类型按 JS 安全范围选**。地址用 `string` 而非 `uint64`——JS 的 number 只精确到 2⁵³，标定地址超限就静默错位；`MAX_CTO`/`MAX_DTO` 这类窄整数放宽到 `uint32`，不为"proto 没有 uint16"在两侧反复转换。
3. **域类型不直接暴露 wire 类型**。IR、会话状态等带不变量的类型留在 `cal-core`，用 `From` 转换；proto 类型只承担对外输出与请求。L1 全公开针对的是契约本身，不意味内部模型要长成 proto 的形状（[D-20](decisions.md#d-20-公开面分层承诺)）。

### 后置项

文档先记结论、实现按触发条件推进——"后置"必须写明原因与到期条件，否则会变成隐性偷懒。下表的全部行都按上面四条判据得出：没有消费者、或依赖重量差不足、或不满足裁剪需求。

| 后置项 | 现在不做的原因 | 触发实施的条件 | 到期未做的后果 |
|---|---|---|---|
| `cal-core` 的 IR 与 `CalibrationSource` | 形状由 ELF 与 A2L 两份输入共同决定，M1 一个输入源都没有 | M2 引入 A2L 或 M3 引入 ELF，出现第一个真实消费者 | 无硬伤；反向代价是提前定形状必然整层重写 |
| `cal-core` 的 quirk 扩展点 | 无真实怪癖样本，trait 形状只能靠猜 | 首次遇到需配置兼容的 ECU（预计 M5） | 怪癖只能改代码，违背 D-06 的开闭要求 |
| `proto` crate | M2 前没有第二个消费者 | UI 与 CLI 同时需要同一套 schema（M2 落 gRPC） | 两处各自定义结构，开始漂移 |
| 独立 `assembly` crate | 只有一个总线后端，一个 `match` 放 `session` 里就够 | 出现第二种重后端，且确有裁剪编译的需求；**M3 已提前落地**（交叉校验需要同时指名两个 format 后端） | 无硬伤，`session` 先变臃肿 |
| `format-elf` / `format-a2l` | M1 的读写按裸地址进行，没有信息源消费者 | 分别随 M3 / M2 引入变量模型时（**均已落地**） | 变量只能手填地址，无自动建立 |
| `format-dbc` / `format-cdf` / `format-mf4` | 各自能力的里程碑未到 | DBC 显示 M3（**已落地**）；dataset 模型 M4（**已落地**，在 `cal-core`）；CDF 导入 M4 **已落地**（按 2.0.0 口径，原 DCM 改判，[D-24](decisions.md#d-24-数据集对接格式改判-cdf)）；MF4 录制与离线重开 M5 | 对应能力缺失：外部标定值暂时只能经 JSON dataset 进出 |
| `daemon` / `ui` | 没有 UI 就没有常驻进程的消费者 | M2 的 Web 最小曲线页 | CLI 已证明引擎可用，但产品形态缺失 |
| `ui-tauri` 桌面壳 | 界面与 Web UI 完全同源，壳只解决"双击启动"这一件事 | 用户明确要单机双击形态（MVP 之后），见 [decisions.md](decisions.md#d-21-native-gui-决策逆转) | 本地使用须先起 daemon，体验差但功能不缺 |
| 前置网关与自研 `gateway` | nginx / Caddy 已覆盖反代、认证与 TLS，daemon 侧零代码 | 车载盒子放不下静态二进制，或需 ACL 直接读工程配置，见 [13-deployment.md](13-deployment.md#自研精简网关的触发条件) | 只能用现成网关，部署门槛落在运维能力上 |
| `bus-zlg` / `bus-socketcand` / `bus-replay` / `bus-vector` | vcan 已能覆盖 M1 的全部验证需求；`bus-zlg` 除外——档三真机抽验需要 Windows 侧后端 | ZLG 真机 M4 末尾（[decisions.md](decisions.md#d-23-真机抽验排期)）；socketcand 与回放按用户请求；Vector 按实际硬件到手 | 真机验证缺位——M1 建抽验清单正是为了提前暴露这一点 |
| `sim-ecu` 的怪癖注入 | 协议层刚自研完，怪癖需真实样本才能定义 | 同 `cal-core` 的 quirk 扩展点 | 模拟器只能覆盖标准行为 |
| `diag` 的黑匣子与指标 | M1 只有日志与最小故障码集，尚无高频 DAQ 可测 | M2 DAQ 上流后 | 偶发问题在现场无法复现 |

### M1 的 crate 集合

M1 只建九个：`diag`、`cal-core`、`xcp-proto`、`bus`、`bus-socketcan`、`xcp`、`session`、`sim-ecu`、`cli`。

集合形状由两条判据直接决定：

- `xcp-proto` 在 M1 即成立而非后置——主从两侧同时出现（`xcp` 与 `sim-ecu`），已满足"第二个消费者"。
- `bus-socketcan` 是 `bus` 的第一个实现：vcan 能完整覆盖 M1 的验证需求，不需要第二个后端，故"裁剪编译"判据此时不成立。

`cal-core` 在 M1 **只放基础类型**：地址、长度、句柄等 newtype，其余全部后置。IR（变量树、Curve/Map/轴）、`CalibrationSource` trait、quirk 扩展点要等真实输入源，M1 的 XCP 读写按裸地址进行。

L4 的 `assembly` 于 M3 落地（交叉校验），仍有大量职责待填；其余 crate 与模块的到期条件见上表。

## 接口一致性

| 手段 | 做法 |
|---|---|
| 类型单一源 | 地址、长度、句柄等基础类型一律用 `cal-core` 的 newtype，禁止各 crate 自定义等价类型 |
| 分发方式 | 跨 crate 边界一律对象安全 trait（`dyn`），crate 内部才用泛型；避免泛型参数污染公开签名 |
| 错误分层 | 每 crate 自有 `Error`，但必须可转 `diag::Fault`；跨边界不传 `anyhow`，只在二进制外壳里用 |
| trait 演进 | `#[non_exhaustive]` 加默认方法，加能力不改签名 |
| 公开面漂移 | `cargo semver-checks` 比对公开 API，破坏性变更必须显式（公开面稳定后启用） |

## 双前端共享引擎

两种使用形态不是两套实现，而是同一个 `session` 引擎的两个外壳：

| 形态 | 外壳 | 特征 |
|---|---|---|
| 持续作业 | `daemon` | 常驻、持有设备与会话、gRPC 推流、静态托管 UI |
| 单次探测 | `cli` | 开设备、做一件事、关设备、输出，进程即退 |

编排、DAQ、录制、状态机全部在 `session` 内，前端只负责 I/O 与生命周期。

### CLI 形态纪律

1. **一条命令即一次自足事务**：不写锁、不留后台状态，同一命令重复执行结果稳定。
2. **分阶段可组合**：`probe bus`（硬件在不在）、`probe xcp`（CONNECT 通不通）、`probe source`（ELF 或 A2L 能否加载）、`probe ecu`（内存读回）各自独立，前一步失败不掩盖后一步的信息。
3. **输出双模**：默认人类可读；`--json` 输出与 `proto` 同源的稳定结构，退出码按故障类别分段（见下节"故障统一"）。
4. **超时内建**：探测硬件超时是常态，默认超时加显式覆盖，绝不无限等待。

这四条服务于同一目的：让外部脚本与 AI 代理以"调一条命令、读一段结构化结果"的方式循环推进，不必解析散文输出。

### 设备独占

CAN 硬件通常不允许两个进程同时打开同一通道。`session` 打开设备时按（后端、通道）加进程级锁，冲突时返回明确故障而非底层错误；`cli` 另提供经本地 gRPC 转发的执行路径（`--via-daemon`），避免为了看一眼现象先停掉正在作业的 daemon。

daemon 的对外边界只四条：明文 HTTP/1.1、默认仅绑 `127.0.0.1`、静态页与 gRPC 同端口、提供 `/healthz`。认证、远程接入与方法级权限不在 daemon 内实现，全部交给前置网关（[decisions.md](decisions.md#d-22-远程接入由前置网关承担)），部署形态与配置示例见 [13-deployment.md](13-deployment.md)。

## 可观测层

日志、故障、指标共用同一套上下文（会话、ECU、span）。分三个 crate 会带来三份初始化与三次依赖声明，故合并为 L0 的 `diag`，内部三个模块。

### 日志三层

| 层 | 内容 | 出口 |
|---|---|---|
| 常规 | 人类可读运行日志，告警以上默认开启 | stderr 与滚动文件 |
| 维测 | 每条 XCP 命令的帧级记录、状态迁移、时序 | JSONL，运行时开关，无需重启 |
| 黑匣子 | 常驻环形缓冲，保留最近一段时间的帧与命令序列 | 故障触发时自动落盘 |

黑匣子针对现场偶发问题：ECU 侧现象事后难以复现，故障瞬间把前一段时间的输入与命令序列一并留下，比事后追加日志开关有效。

### 故障统一

单一 `Fault` 结构承载：稳定编号、严重度、上下文、可操作建议、原始错误链。编号按域分段（硬件、协议、信息源、会话、接口、内部），`cli` 退出码与编号段一致。

**`FaultClassifier` 是唯一允许判别后端差异的位置**：底层 `io::Error`、socketcan 错误、协议层错误、超时全部在这里归一化；上层只用 `?` 传递自己的错误。

上报走 `FaultSink` 多实现：日志、gRPC 推 UI 故障面板、`cli` 输出、可选外部上报（默认关闭，需显式开启）。UI 侧只有一个统一故障面板，不做分散提示。

系统自检（`doctor`）与分阶段探测是同一套实现的两个粒度：`doctor` 把所有故障类别的判定条件跑一遍。

### 指标

指标与日志共用 span，但不共用输出通道。必须是一等指标的只有五项：

- DAQ 帧率
- **丢帧数**（高频采集最先崩的地方）
- XCP 命令延迟分位（p50 与 p99）
- 背压排队深度
- 进程内存

出口：UI 性能面板、`cli` 的实时视图、周期性快照落盘供事后分析。

## 公开面策略

不是全公开，也不是全隐藏——按层承诺：

| 层 | 公开面 | 承诺 |
|---|---|---|
| L1 契约 | 全部公开 | semver 严格，1.0 后不破坏 |
| L2 实现 | 仅 trait 实现与少量构造器，内部模块 `pub(crate)` | 允许重构内部 |
| L3 编排 | 一个会话 facade 加配置结构与事件枚举，字段全私有 | 改内部不改签名 |
| L5 外壳 | 二进制，不对外 | — |

四条机制：

1. **Facade 手势**：对外只给结构与放行方法，字段私有、构造走 `new`/Builder。私有字段意味着内部结构调整不算破坏性变更。
2. **扩展点在 trait，不在函数**：接新硬件实现 `Bus`、接新信息源实现 `CalibrationSource`、调 ECU 怪癖走外部配置。能扩展的地方必须能以 `dyn` 取出——这是"因抽象不合理或不公开导致别人用不起来"的根治手段。
3. **`pub mod unstable`**：半成品放这里并声明不保证兼容，既避免憋着不公开，也避免全公开带来的承诺负担。
4. **`examples/` 是抽象的验收标准**：每个契约 crate 至少一个只用公开 API 的完整小流程（读 ELF 打变量、开 socketcan 收帧、起 sim-ecu 与 master 连上）。写不出 example 即抽象不足，这比代码评审更能暴露问题。

反向规则：**不为"将来可能有用"而公开**。判据是有真实调用方或 example 需要，否则先 `pub(crate)`——将来放开是兼容变更，反过来收紧则是破坏性变更。

## 代码组织

```
crates/
  diag/src/{lib,fault,log,metrics}
  cal-core/src/{lib,ir,traits,quirk}
  xcp-proto/src/{lib,cto,dto,daq,wire}
  bus/src/{lib,traits,capabilities}
  xcp/src/{lib,connection,page,daq}
  session/src/{lib,engine,recorder,lock}
  cli/src/{main,cmd}
```

每个 crate 固定四段：`lib.rs` 只做 re-export 与 crate 文档、不放实现逻辑；另有 `error.rs`、可选的 `traits.rs`、实现子模块。`lib.rs` 的形状即该 crate 对外承诺的形状。

## 参考实现与致谢

借鉴而不依赖的上游（典型如 XCP master 的命令序列、DWARF 遍历方式）统一登记在 [credits.md](credits.md)，分"依赖 / 行为参考 / 格式规范"三类，每条注明许可证与借鉴的具体内容；实质借鉴的文件头加来源与许可证注释。

这是"参照实现而非引入依赖"的合规底线：不引入上游代码不等于可以不留出处。
