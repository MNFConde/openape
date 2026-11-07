# 测试夹具

入库的测试输入与期望输出，供 M1 起的单元测试与 M3 的档二 golden 使用。命名与目录划分随实现推进扩展，当前是 M0 spike 期沉淀的两组。

| 目录 | 内容 | 来源 |
|---|---|---|
| `elf/` | 覆盖标定/观测典型形态的 C 源码（常量、二维 Map、双轴、结构体、位域、枚举、指针、函数指针、跨 CU 同名 static、被优化折叠的 static const），以及编好的 `test.elf` 与 `stripped.elf` | M0-4 spike |
| `dbc/` | CAN 数据库夹具（`typical.dbc`：Intel/Motorola、有符号、带单位） | M3-08 |
| `config/` | 轴绑定 golden 的工程配置（`axis-binding-golden.toml`）与期望 IR 快照（`.ir.json`） | M3-06 |
| `a2l/` | 轴绑定导出的 golden A2L 片段（`axis-binding-golden.a2l`）与档二 golden（`elf-export-golden.a2l`：`test.elf` 全量导出） | M0-5 RFC 第 8 节 / M3-05 |
| `cdf/` | CDF 档二 golden 三件套：输入样本（`minimal.cdf`）、变量名配置（`golden.names.toml`）、期望导入结果（`golden.import.json`） | M4-06 / M4-07 |

## ELF 夹具：产物入库，源码随改随编

`elf/test.elf` **入库**。这不是图省事——A2L 导出里含地址与偏移，它们完全由编译器和优化等级决定，gcc 换个版本、换个编译目录，字节就变了。若测试现编，任何机器升级都会把 golden 变成红灯，而红灯本身不说明代码有问题。产物入库后，测试读的是同一个二进制，结论只随代码变。

代价是**源码与产物必须同步**：改了 `fixtures/elf/*.c` 就要重编并一起提交，否则测试验证的是一份过时的声明。重编命令：

```bash
cd fixtures/elf
gcc -c -g -O2 -o calib.o calib.c
gcc -c -g -O2 -o other.o other.c
gcc -c -g -O2 -o main.o main.c
gcc -o test.elf calib.o other.o main.o
cp test.elf stripped.elf && strip stripped.elf
rm -f *.o
```

`stripped.elf` 是同一份二进制去掉调试信息后的样子，用来钉住"体检提示要说人话"这条验收。

`-g` 与 `-O2` 同时给是刻意的：`-g` 才有 DWARF，`-O2` 才能复现"变量被折叠掉"的场景（`s_folded` 在符号表与 DWARF 中都不存在）。

产物可复现的前提是**在同一目录下重编**：DWARF 里记录编译单元路径（`DW_AT_comp_dir`），换个目录编出来的字节就不同。当前产物由 gcc 13.3.0（Ubuntu 24.04）编出，md5 `345b9226bf11eb4b7b3172e394ffc58c`。升级 gcc 后重编会得到新字节，属正常，提交时在 message 里说明即可。

`.o` 中间产物不入库。

## A2L 夹具

`a2l/axis-binding-golden.a2l` 是 [10-axis-binding-rfc](../docs/10-axis-binding-rfc.md) 第 8 节的期望导出结果，用 `a2lfile` 严格模式校验（`load(path, None, true)`）应为零警告。

`a2l/elf-export-golden.a2l` 是 `elf/test.elf` 走完"抽 DWARF → 进 IR → 指定角色 → 空轴绑定导出"后的完整输出，由 `crates/format-a2l/tests/elf_to_a2l.rs` 做字节级比对。改了导出格式或换了夹具，这条 golden 必须一起更新，并在提交里说明是哪一种。

## CDF 夹具：样本自造，期望结果逐年可重生成

`cdf/minimal.cdf` **是本项目手写的**，不是从工具链导出的真实文件。原因有两条：ASAM 官网的 CDF 2.1.0 下载只对会员开放，公开可得的是 2.0.0 的 XSD，而该 XSD 的版权属 ASAM、不能进本仓（[D-24](../docs/decisions.md#d-24-数据集对接格式改判-cdf)）；许可干净的公开样例文件也没找到。手写的样本版权清楚，还能随手改造成坏样本去测报错路径。

样本覆盖标量、字符串（`VT`）、曲线、二维 map、结构体成员（嵌套 `SW-INSTANCE`）、带单位与不带单位，以及一个"合法但我们不解析"的 `SW-CS-HISTORY`——它用来钉住"给提示而不是拒"这条行为。

三件套互为约束：

| 文件 | 角色 |
|---|---|
| `cdf/minimal.cdf` | 输入 |
| `cdf/golden.names.toml` | 配置：我们这边的变量名清单，刻意让"对上 / 只在 CDF / 只在我们这边"三桶都非空 |
| `cdf/golden.import.json` | 期望：解析结果 + 三桶对齐，字节级比对 |

重新生成（改了样本、改了投影字段、或改了提示文案时）：

```bash
OPENAPE_WRITE_GOLDEN=1 cargo test -p openape-format-cdf --test golden
```

没有这个开关时只比对不写。改了期望要在提交里说明是哪一种改动（样本／投影／提示），否则"改实现顺手改 golden"会让比对失去意义。

**已知边界**：`format-cdf` 按 CDF 2.0.0 实现，2.1.0 与 2.0.0 的差异未验证；真实工具链导出的文件到手后，应先跑一遍解析、把元素集表里的漏项补齐，再考虑补进 fixtures。
