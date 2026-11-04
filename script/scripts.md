# 脚本台账

统一触发：`python3 script/{name}.py`（unix）；Windows 下 `python` 或 `uv run --no-project python`，`.githooks/commit-msg` 薄壳按 python3 / python / py -3 / uv run 四级自动探测。主开发环境 WSL2 Ubuntu-24.04，python3 直接可用；Windows 侧 python 为商店占位 stub 时走 `uv run --no-project python script/{name}.py`。脚本保持零依赖档（纯标准库，禁引第三方包）。

| 脚本 | 用途 | 门禁挂点 |
|---|---|---|
| [check-links.py](check-links.py) | 校验 docs/（+cairn/）全部 Markdown：跨文件链接目标存在、站内/跨文件锚点与 GitHub slug 一致；递归含子域、跳过 archive_docs_v* | 提交门禁（命令单一源 `.agents/rules/commit.md` 门禁流水线节） |
| [check-frontmatter.py](check-frontmatter.py) | 校验 docs/ 每篇 frontmatter 必填 4 项（type/status/summary/updated）与 status 枚举 | 提交门禁（同上） |
| [check-markers.py](check-markers.py) | 校验方案状态标记两格式（行内简记=链接、块记=小标题+callout）结构 | 提交门禁（同上） |
| [check-commit-msg.py](check-commit-msg.py) | 校验提交信息 Conventional Commits（结构/宽度/行数/footer/豁免清单） | 由 [.githooks/commit-msg](../.githooks/commit-msg) 钩子调用 |
| [../.githooks/commit-msg](../.githooks/commit-msg) | 钩子薄壳：定位仓库根 → 四级探测 python → 调用 check-commit-msg；全不命中时告警放行——该输出 = 门失效须修环境 | git commit 自动 |

## 临时探针台账

> 单/双次使用的诊断代码记一行（日期/内容/用途/次数）；跨会话累计 3 次的探针升级固化进本目录。写探针前先查本表。

（暂无登记。）
