---
type: architecture
status: active
summary: "docs/ 文档地图：OpenAPE 治理文档索引与稳定度分组"
updated: 2026-11-05
---

# docs/ 文档地图

OpenAPE（Rust 开放汽车标定工具）项目文档索引。所有文档为规划稿，随实现推进持续更新。本文件自身也带 frontmatter——`check-frontmatter.py` 对 docs/ 全部 .md 无豁免。

## 按稳定度分组（上层稳定、下层易变；分层放地图，编号只管阅读顺序）

### 上游（做什么 / 不做什么）

| 文档 | 职责 | 何时读 |
|---|---|---|
| [requirements.md](requirements.md) | 需求基线：能力清单、MVP 边界、非功能约束、验收口径 | 一切的起点；扩范围前 |

### 规范（是什么）

| 文档 | 职责 | 何时读 |
|---|---|---|
| [00-overview.md](00-overview.md) | 总纲：定位、workspace 拓扑规划、复用与自研边界、本域索引 | 理解整体结构时 |
| [90-env.md](90-env.md) | 开发环境：uv 管理 Python、vcan 虚拟总线、pyXCP 对照 master | 搭环境/复现实验时 |
| [AGENTS.md](AGENTS.md) | docs/ 写作约束：frontmatter、编号、单一事实源、存档 | 写/改 docs/ 前 |

### 论证（为什么这么选）

| 文档 | 职责 | 何时读 |
|---|---|---|
| [decisions.md](decisions.md) | 决策记录 D-xx（单文件形态） | 遇到"为什么这么选"时 |

> 领域篇（10-70 号段）随设计长出、不预建：首个预告为 `10-` Curve/Map 轴点注记约定 RFC（M0-5）；80-diff 按需占尾号，90 号段已由 [90-env.md](90-env.md) 占用。

**推荐阅读顺序**（新加入者）：根 AGENTS.md → requirements → 00 总纲 → decisions 按需。

## 引用规范

- 全文档引用一律用 **markdown 链接**，不用裸编号：跨文件 `[文本](文件名.md#标题锚点)`；同文件 `[文本](#标题锚点)`
- 锚点 = 标题转小写、去标点、空格 → `-`、中文保留（GitHub slug 规则）；标题不含 `、（）+` 等标点
- 同一事实只一处详述（单一事实源），其余链接回指：需求与边界源在 requirements、选型依据源在 decisions、结构与拓扑源在 00 总纲
- 方案状态标记（`【当前】/【备选】/【已否决】/【已替换】`，行内简记 = 链接、块记 = 小标题 + callout）见 [AGENTS.md](AGENTS.md)
- 锚点一致性检查：`python3 script/check-links.py`（退出码非 0 = 有不匹配；各机器触发细则见 [../script/scripts.md](../script/scripts.md) 台账）

## ADR 登记

- 小项目用 `decisions.md` 单文件形态（每条 `D-xx` 一节）；超过约 10 条拆 `adr/` 目录一篇一文件 + 索引
- 每条必填：**状态**（已决策/待定）/ **日期** / **规范位置**（反向链接）/ **背景** / **决策** / **依据** / **影响**；编号连续递增
- 规范文档凡涉及该决策处，用链接指向对应编号，不重复粘贴论证
- 讨论未决项标"待定"，并列出验证项；状态变化时同步更新正文标记
