# OpenAPE

**Open** Automotive **P**arameter **E**ditor —— an open automotive calibration tool in Rust.

Rust 开发的开放汽车标定工具：XCP-on-CAN 观测量采集与标定量读写、ELF / A2L / DBC 解析、headless daemon + Web UI（本地与远程同一套页面）。设计取向与选型依据见 [docs/](docs/README.md)（中文）。

> 状态：架构规划完成，代码未起步。License：Apache-2.0 OR MIT（双许可）。

## 开发者须知

- 项目规则入口：[AGENTS.md](AGENTS.md)（中文，AI 协作与质量门禁的单一起点）
- 提交前门禁与钩子安装见 `.agents/rules/commit.md`（一次性：`git config core.hooksPath .githooks`）
- Contributor guidelines: read `AGENTS.md` first; commit messages follow Conventional Commits and are hook-enforced.
