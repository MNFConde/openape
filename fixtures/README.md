# 测试夹具

入库的测试输入与期望输出，供 M1 起的单元测试与 M3 的档二 golden 使用。命名与目录划分随实现推进扩展，当前只有 M0 spike 期沉淀的两组。

| 目录 | 内容 | 来源 |
|---|---|---|
| `elf/` | 覆盖标定/观测典型形态的 C 源码（常量、二维 Map、双轴、结构体、位域、枚举、跨 CU 同名 static、被优化折叠的 static const） | M0-4 spike |
| `a2l/` | 轴绑定导出的 golden A2L 片段 | M0-5 RFC 第 8 节 |

## 编译 ELF 夹具

```bash
cd fixtures/elf
gcc -c -g -O2 -o calib.o calib.c
gcc -c -g -O2 -o other.o other.c
gcc -c -g -O2 -o main.o main.c
gcc -o test.elf calib.o other.o main.o
```

`-g` 与 `-O2` 同时给是刻意的：`-g` 才有 DWARF，`-O2` 才能复现"变量被折叠掉"的场景（`s_folded` 在符号表与 DWARF 中都不存在）。

`.o`/`.elf` 产物不入库，由测试自行编译——避免二进制与源码版本漂移。

## A2L 夹具

`a2l/axis-binding-golden.a2l` 是 [10-axis-binding-rfc](../docs/10-axis-binding-rfc.md) 第 8 节的期望导出结果，用 `a2lfile` 严格模式校验（`load(path, None, true)`）应为零警告。
