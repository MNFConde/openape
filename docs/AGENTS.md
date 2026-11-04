---
type: rule
status: active
summary: "docs/ 写作约束：frontmatter、编号、单一事实源、存档"
updated: 2026-11-04
---

# AGENTS.md

docs/ 目录约定（本文件只约束 docs/ 内的文档写作与组织；根 AGENTS.md 不复述此处内容，读取本目录文件时自动生效）。

## docs/ 目录约定

- 归属：docs/ 只收架构、实现、需求相关文档；协作约定、工作流不入 docs/（放 `.agents/rules/`）。
- 组织：编号 + frontmatter——00 总纲 / 10-70 领域篇随设计长出（十位一层、留空洞）/ 80-diff、90-env 占尾号；每篇必带 frontmatter（必填 type / status / summary / updated，含本目录 README.md 地图，机器校验无豁免）。
- 统一校验入口：`python3 script/check-links.py`、`python3 script/check-frontmatter.py`、`python3 script/check-markers.py`（挂提交门禁，命令单一源在 `.agents/rules/commit.md` 门禁流水线节）。
- 台账/索引：文档地图在 [README.md](README.md)，与实际文件一一对应；新增文档必须先登记进地图。

## 特有法则

- 单一事实源：同一事实只一处详述——需求与边界的源在 requirements、选型依据的源在 decisions、结构与拓扑的源在 00 总纲；其余出现处一律链接回指。
- 文档标题不含 `、（）+` 等标点，保锚点干净；引用一律 markdown 链接，锚点按 GitHub slug 规则。
- 方案状态标记（`【当前】/【备选】/【已否决】/【已替换】`）行内简记必为链接、块记必为小标题 + callout，两格式标签文字一字不差；标记内嵌在行内反引号中为豁免（解释语法不算使用）。
- 结构重组 / 整篇重写**前**先存档：新建 `archive_docs_v{num}/`（num = 现有最大编号 + 1），快照复制本目录顶层全部文件（不含既有存档目录），复制完再调整；旧存档永不删除、不再修改，检查脚本自动跳过。

## 经验与坑（必选登记）

> 本目录相关的教训必须在此登记一份精简版（坑/对策/校验/详情指针）；详情另存 cairn/ 知识专题文档并附指针，不得只沉淀在会话记忆里。

<!-- 格式：
### N. <一句话坑名>
- 坑：<现象与根因>
- 对策：<具体做法>
- 校验：<如何确认没踩>
- 详情：<指针 + 实证日期>
-->

（暂无登记。）
