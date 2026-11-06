# 提交信息格式规范（Conventional Commits）

> 作用范围：本仓库所有提交。安装钩子（一次性，本地配置不入库）：`git config core.hooksPath .githooks`

> 本文件为提交相关经验/坑的**必选登记处**：后续任何提交相关教训，必须在本文件
> 「经验与坑」节登记精简版；详情另存 cairn/ 知识专题文档并附指针（沉淀层已初始化），不得只沉淀在会话记忆里。

## 一、标准完整格式

```
<类型>(<可选范围>): <主题描述>

<可选正文：为什么这么改，改动了什么逻辑，解决了什么问题>

<可选脚注：BREAKING CHANGE: <描述> / Closes #123, #456>
```

## 二、核心类型速查表

| 类型 | 含义 | 版本号 |
|---|---|---|
| feat | 新增功能 | MINOR |
| fix | 修复 Bug | PATCH |
| docs | 仅文档 | — |
| style | 格式调整，不影响逻辑 | — |
| refactor | 既非修 Bug 也非加功能 | — |
| perf | 性能优化 | PATCH |
| test | 测试用例 | — |
| build | 构建系统/外部依赖 | — |
| ci | CI/CD 配置 | — |
| chore | 杂项，不涉及 src/test | — |
| revert | 回滚 | — |

## 三、编写规范

**主题行**：类型开头 + 冒号空格；祈使句/现在时；结尾无句号；不以 ASCII 大写字母/编号开头；整行 ≤72 显示列（全角按 2 列计）。

**正文**：解释"为什么改"和"怎么改的"；每行 ≤72 显示列；有效行 ≤20 行（脚注段不计入）——超过通常意味着该拆分提交，或先把根因/论证沉淀到 `docs/` 后附指针。

## 四、钩子强制项 vs 语义人工项

- **钩子强制**：结构、类型白名单、冒号后空格、主题非空/无句号/首字符规则、显示宽度、正文有效行数、`BREAKING CHANGE:`/`Closes #` 格式、空信息拦截；豁免 `Merge`/`Revert`/`fixup!`/`squash!` 与 `#` 注释行。
- **语义人工项**：类型选型是否准确、是否真在解释"为什么"、是否真算 BREAKING——靠提交者判断与 review。

## 五、质量门禁流水线（命令单一源）

> 本节是门禁命令的**唯一源**；根 AGENTS.md、docs/ 地图、台账只指针回指。流水线固定顺序：格式化 → 静态零警告 → 测试 → 安全审计，再加文档三检查。

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo audit
python3 script/check-links.py
python3 script/check-frontmatter.py
python3 script/check-markers.py
```

- cargo 四条自 workspace 脚手架落地（todo M1）起生效；此前文档三检查即门禁。
- 触发细则（python3 / python / py -3 / uv run 四级探测，Windows 商店占位 stub 问题）单一源见 [script/scripts.md](../../script/scripts.md) 台账——本开发机实际执行 `uv run --no-project python script/{name}.py`。
- 版本约束只钉一处（workspace 根 Cargo.toml 的 workspace.dependencies）；升级后必须跑全量测试 + `cargo audit`，勿随手升级。
- **批量机械改写后跑未使用符号扫描**：perl/sed/脚本批量改写会静默丢测试属性/函数——`cargo clippy --all-targets` 的 dead_code 警告即探测器，改写后该类警告必须清零。
- 提交信息门禁 = commit-msg 钩子（安装与活性证明见「钩子安装」节）。
- **前端构建检查不进本流水线、也不挂提交钩子**：`npm ci && tsc --noEmit && vite build` 耗时长且依赖本地 node 环境，挂 pre-commit 只会逼人用 `--no-verify`，连带绕过 commit-msg 校验。以独立脚本提供，有 CI 后挂 CI（口径见 [docs/13-deployment.md](../../docs/13-deployment.md#前端构建与产物)）。

## 六、提交前自检清单

- [ ] 首行 `类型: 主题`，冒号后带空格，类型与改动性质匹配
- [ ] 主题为祈使句/现在时，结尾无句号，不以 ASCII 大写/编号开头
- [ ] 主题行 ≤72 显示列；正文解释了"为什么"，每行 ≤72 列、有效行 ≤20 行
- [ ] 破坏性变更已注明 `BREAKING CHANGE: <描述>`；关联 Issue 用 `Closes #xxx`
- [ ] 质量门禁全绿（第五节流水线）
- [ ] CJK 写入完整性闸已过（第七节）
- [ ] 本轮做过批量机械改写（perl/sed/脚本）：未使用符号扫描已跑且清零
- [ ] 产物有变更时，验收口径适用项已过（「验收口径」节）
- [ ] 提交后核对钩子确有通过输出（无输出或"跳过格式校验" = 门没活着，查 `core.hooksPath`、执行位与 python 环境）

## 七、非 ASCII 写入完整性（启用：本项目为 CJK 项目）

AI 侧写入偶发把 CJK 字符坏成 U+FFFD 替换符，且能静默混进提交（肉眼扫 diff 难察觉）。三件防线：

- **提交信息一律走文件**：`git commit -F <file>`，不用命令行内联（内联串不可 grep、坏字符不可见）。
- **提交前过闸**：`grep -c $'\xEF\xBF\xBD'` 扫全部改动文件与提交信息文件，**零命中才放行**。文档里引用该符号一律写转义形式 `\xEF\xBF\xBD`、不写字面替换符——否则闸门永远非零、形同虚设。
- **命中按字节级修复**：按 UTF-8 字节序列匹配替换（如 perl `-pe` 字节替换），不做编码层转换（转换会引入二次歧义）。

## 八、提交相关经验与坑（必选登记）

> 提交相关教训必须在本节登记精简版（详情另存 cairn/ 知识专题文档并附指针），供提交时一眼可见。

### 1. Windows 商店 python 占位 stub 骗过探测

- 坑：商店占位 python.exe 会被 `command -v` 找到但一跑就败（本开发机 python/python3 即如此，py 未装），探测误判"有 python"而钩子实际不执行校验。
- 对策：钩子薄壳探测用 `$cand -c "import sys"` 实跑判定，不只看 `command -v`；本开发机依赖第 4 级 `uv run --no-project python` 探测命中。
- 校验：装完钩子立即做活性双探针（坏信息必须被拒、好信息必须通过），见「钩子安装」节。
- 详情：就地展开（commit-msg 薄壳注释；2026-11-04 初始化时本机实测）。

### 2. 钩子 sh 文件被 CRLF 破坏

- 坑：Windows autocrlf 环境下 checkout 把 sh 钩子行尾转成 CRLF，sh 解析失败或行为异常，格式门静默失效。
- 对策：`.gitattributes` 钉 `.githooks/commit-msg text eol=lf`。
- 校验：`grep -c $'\r' .githooks/commit-msg` 为零。
- 详情：就地展开（`.gitattributes` 注释）。

### 3. commit -m 内联信息不可审

- 坑：内联在命令行上的提交信息不落文件，CJK 坏字符不可 grep、不可见，能静默进历史。
- 对策：提交信息一律 `git commit -F <file>`；提交前对改动文件与信息文件跑 U+FFFD 零命中闸。
- 校验：`grep -c $'\xEF\xBF\xBD' <files>` 零命中才提交。
- 详情：见「非 ASCII 写入完整性」节（就地承担完整职能）。

### 4. 正文写成超长整句会被钩子拒

- 坑：AI 侧习惯把"为什么改"写成一条不断行的长句；钩子按全角 2 列算显示宽度，单行 137 列直接被拒（`格式错误：正文第 2 行显示宽度 137 列，超过 72 列`），提交失败但容易误以为已提交。
- 对策：提交信息先写文件，正文手动按 ≤72 列断行；提交前用 `.githooks/commit-msg <file>` 预检，再 `git commit -F`。
- 校验：预检输出「提交信息格式校验通过」；`git log --oneline -1` 确认新提交存在。
- 详情：就地展开（2026-11-05 M0-2 文档提交实测，重排后才通过）。

### 5. 主题里的英文词首字母大写会被钩子拒

- 坑：主题写成 `docs: M0-3 spike 结论落文档`，钩子报「主题英文首字母不要大写」——它检查的是主题中**首个英文词**的首字母，里程碑编号（`M0-3`、`D-13`）与专名（`Rust`、`OpenAPE`）都在管辖范围内，唯一豁免是放在括号里（`（D-13）` 实测通过）。
- 对策：主题以中文开头，编号与专名放进括号或改成小写（`docs: 记录 m0-3 spike 结论（grpc-web 流式可行）`）。
- 校验：`.githooks/commit-msg <file>` 预检通过后再 `git commit -F`。
- 详情：就地展开（2026-11-05 M0-3 提交实测，首次被拒后改主题才通过）。

## 九、钩子安装

一次性执行（本地配置，不入库，不自动推送）：

```
git config core.hooksPath .githooks
```

装完立即做**活性双探针**（钩子探测 python 失败时会静默放行，装了 ≠ 活着）：

```
echo "bad message no type prefix" > /tmp/probe-bad.txt
echo "docs: 活性探针" > /tmp/probe-good.txt
.githooks/commit-msg /tmp/probe-bad.txt    # 期望非零退出 + 格式错误输出（拒）
.githooks/commit-msg /tmp/probe-good.txt   # 期望零退出 + 校验通过输出（过）
```

（用消息文件而非管道——git 实际就以文件路径调钩子，且管道 stdin 在部分环境不可读。）

## 十、验收口径（产物层，按条件启用）

> 代码层门禁见第五节；本节管**产物**对不对，判定与细则依据见 vibe-project-init `references/acceptance.md`。本项目按档判定结果：

- [ ] 档一·权威校验器（**适用，M3 起**）：A2L 导出件用 a2lfile 重新解析 + 版本降级检查，零 fatal 零 error 才算过
- [ ] 档二·golden 基线（**适用，M3 落地**）：ELF→A2L 导出与 DCM 解析做字节级 golden 测试，fixtures 入库；更新 golden 须人工确认 diff；纯重构以字节零 diff 为通过线
- [ ] 档三·人工抽验（**适用，M1 起建清单**）：里程碑合入前真机抽验——周立功 CAN-FD 卡 + 目标 ECU，检查点矩阵随 M1 产出；发现问题按"发现 → 固化进 fixtures/golden → 门禁覆盖"回路回灌
- 档四·决策日志：**不适用**——无运行时判定通道；条件成立时按 `references/acceptance.md` 启用
