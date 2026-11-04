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

## 经验与坑（必选登记）

### 1. pyXCP 在中文时区直接崩

- 坑：pyXCP 把 `time.tzname` 喂给 `pytz.timezone()`，而 Asia/Shanghai 的缩写是 `CST`，pytz 只认 IANA 名，抛 `UnknownTimeZoneError: 'CST'`；连 `TZ=Asia/Shanghai` 也逃不掉（缩写仍是 CST）。
- 对策：跑 pyXCP 脚本时前置 `TZ=UTC`。
- 校验：`TZ=UTC python -c "import time; print(time.tzname)"` 输出 `('UTC','UTC')`。
- 详情：就地展开（2026-11-05 M0-2 spike 实测）。

## 相关

- 决策依据：[decisions.md](decisions.md)
- 脚本触发方式单一源：[../script/scripts.md](../script/scripts.md)
