---
type: environment
status: active
summary: "开发环境：WSL2 + uv 管理 Python + vcan/can-utils + pyXCP 对照 master"
updated: 2026-11-05
---

# 开发环境

本机开发环境为 WSL2（Ubuntu 24.04，内核 6.18 系 microsoft-standard-WSL2）；Windows 侧保留周立功 CAN-FD 卡的真机验证。本文只记**可复现的环境准备**，不含业务逻辑；sim-ecu 侧选型见 [decisions.md](decisions.md#d-12-xcp-on-can-从站自研)。

## Python 一律由 uv 管理

- 安装：`curl -Lsf https://astral.sh/uv/install.sh -o tmp/uv-install.sh && sh tmp/uv-install.sh`（落到 `~/.local/bin`，与 `skills-manager-cli` 同目录）
- 脚本校验直接跑，不建环境：`uv run --no-project python script/check-links.py`（触发细则见 [../script/scripts.md](../script/scripts.md)）
- 需要第三方包时用项目内虚拟环境，不用系统 pip：

```bash
uv venv .venv
uv pip install --python .venv/bin/python <packages>
```

## vcan 虚拟总线

内核自带 `vcan` 模块，不需重编；`can-utils` 提供 `candump` / `cansend` 用于抓包核对。

```bash
apt-get install -y kmod can-utils
modprobe vcan
ip link add dev vcan0 type vcan
ip link set up vcan0
candump vcan0 &          # 抓包
cansend vcan0 123#DEADBEEF
```

CAN 侧约定：CMD 帧 0x600（master→slave）、RES 帧 0x601（slave→master）；XCP 帧 8 字节，PID 在 byte0。

## XCP 对照 master

自研从站不能自证，用第三方 master 做对照：pyXCP（CANape 同源生态）+ python-can。

```bash
uv venv .venv
uv pip install --python .venv/bin/python pyxcp==0.30.2 python-can==4.6.1
```

## 本机 agent 工具链

- **skills-manager CLI**（`~/.local/bin/skills-manager-cli`）：技能库在 `~/.skills-manager/skills`，对远程 `skills-manager-backup` 仓库只做**只读镜像**——push 地址指向 `DISABLED_PULL_ONLY`，推送被物理阻断；同步入口 `sm-sync`（fetch → `reset --hard` → `clean -fd` → CLI 索引重建 → 死链检查），本地改动一律丢弃。
- **技能加载目录**：OpenBitFun 扫描项目级 `.agents/skills`（另有 `.claude/`、`.codex/`、`.cursor/`、`.opencode/`、`.openbitfun/` 等），且**跟随符号链接**；home 级目录不被本项目使用。
- **接线方式**：`skills-manager-cli agents add-custom openbitfun-project --path <repo>/.agents/skills`，再 `skills deploy <skill> --agent openbitfun-project` 生成符号链接，指向库内实体。
- **仓库屏蔽**：`.git/info/exclude` 加 `/.agents/skills/`（本地私有，不入库也不随仓库分发），`git status` 保持干净。

## 经验与坑（必选登记）

### 1. pyXCP 在中文时区直接崩

- 坑：pyXCP 把 `time.tzname` 喂给 `pytz.timezone()`，而 Asia/Shanghai 的缩写是 `CST`，pytz 只认 IANA 名，抛 `UnknownTimeZoneError: 'CST'`；连 `TZ=Asia/Shanghai` 也逃不掉（缩写仍是 CST）。
- 对策：跑 pyXCP 脚本时前置 `TZ=UTC`。
- 校验：`TZ=UTC python -c "import time; print(time.tzname)"` 输出 `('UTC','UTC')`。
- 详情：就地展开（2026-11-05 M0-2 spike 实测）。

### 2. 只读镜像会让部署的符号链接变死链

- 坑：`sm-sync` 在库内执行 `reset --hard` + `clean -fd`，远端删除或改名某个 skill 后，`.agents/skills` 里指向它的符号链接就成了死链，`git status` 看不出来（目录已 ignore），症状是 skill 静默消失。
- 对策：`sm-sync` 末尾遍历各 agent 的 skills_dir 检查链接目标是否存在，报出 agent / 链接路径 / 目标路径并以非零退出码提示。
- 校验：`sm-sync` 输出「ok: N 个符号链接全部有效」；出现死链时按提示 `skills undeploy` 后重新 deploy。
- 详情：就地展开（2026-11-05 实测：造死链能被精确报出，正常态 12/12 有效）。

## 相关

- 决策依据：[decisions.md](decisions.md)
- 脚本触发方式单一源：[../script/scripts.md](../script/scripts.md)
