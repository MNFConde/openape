---
type: rfc
status: draft
summary: "Curve/Map 轴点注记约定：轴为独立实体、按维度顺序引用、用户绑定配置格式与校验规则，含 golden 样例"
updated: 2026-11-05
---

# Curve 与 Map 轴点注记约定

> 状态：draft（设计已定，待 M1 的 IR 与解析落地后转 active）。范围按 [D-13](decisions.md#d-13-信息源两档输入与写入安全) 收窄：只定**轴的建模、引用与用户绑定**，不定单位与物理换算。

## 1. 问题

DWARF 里 `float cal_map[4][3]` 只是"4×3 的 float 数组"——它不表达"哪一维是温度轴、轴值存在哪、轴上一共几个点"。A2L 能表达（`AXIS_PTS` 加 `AXIS_DESCR`/`AXIS_PTS_REF`），但那是人填的。

仅 ELF 档没有语义来源（D-13 明确不做启发式），所以必须有一份**人能写、能 diff、能版本化**的绑定配置。本 RFC 定义这份配置的格式、内部模型与校验规则。

不在本文范围：单位、量程、物理换算（归 A2L 档）、标定与观测角色判定（D-13 由用户指定）、自定义 section 注记（推迟到 M3，届时直接复用本文模型）。

## 2. 轴作为独立实体

轴不是值的附属属性，而是可被多方引用的实体。这条建模决定了一个 Map 与多个 Map 共用同一条 X 轴时不需要复制数据（对应 A2L 的 `COM_AXIS`）。

| 字段 | 含义 | 来源 |
|---|---|---|
| `id` | 工程内唯一标识，配置中用于引用 | 配置 |
| `symbol` | 变量符号路径，可含成员（如 `cal_gains.kp`） | 配置 |
| `len` | 轴点数，硬校验锚点 | DWARF 的 `dim[0]` 或 A2L 的 `max_axis_points` |
| `kind` | `shared`（独立存放）或 `inline`（内嵌在值数组里） | 配置，默认 `shared` |

值实体（Curve 与 Map 同构，只是维度数不同）：

| 字段 | 含义 | 来源 |
|---|---|---|
| `id` | 工程内唯一标识 | 配置 |
| `symbol` | 值数组符号路径 | 配置 |
| `dims` | 各维长度 | DWARF 的 `dim` |
| `axes` | 轴引用列表，长度必须等于 `dims` 长度 | 配置 |
| `order` | 维度顺序，见第 3 节 | 由 `dims` 与 `axes` 共同确定 |

## 3. 维度顺序约定

**配置里 `axes` 的顺序与 DWARF `dim` 的顺序一致**：`axes[i]` 对应 `dim[i]`。

```c
float cal_map[4][3];   /* dim = [4, 3] → axes = [温度轴(4 点), 速度轴(3 点)] */
```

理由：配置是面向 ELF 写的，直接对应 `dim` 最不容易错；把逆序转换集中到「导出 A2L」一处（第 7 节），单点可测。

**方阵是这套约定的盲区**：`float m[4][4]` 时顺序写反不会触发长度校验，只能靠 golden 与人工复核发现。因此 golden 样例必须包含一个非方阵 Map——本文第 6 节即是。

## 4. 配置格式

工程配置统一用 TOML，本 RFC 只占其中三节，其余节由后续设计扩展（解析器须**容忍未知字段**，遇到未知 `schema_version` 则报错）。

```toml
schema_version = 1

[[axis]]
id = "x_speed"
symbol = "cal_axis_x"
kind = "shared"

[[axis]]
id = "y_temp"
symbol = "cal_axis_y"

[[curve]]
id = "limiter"
symbol = "cal_limiter"
axes = ["x_speed"]

[[map]]
id = "eff_map"
symbol = "cal_map"
axes = ["y_temp", "x_speed"]
```

约定：

- `id` 在工程内全局唯一（轴与值共用命名空间），不得与任何符号路径重名
- `kind` 缺省为 `shared`；`inline` 只在 M3 导出 A2L 的 `STD_AXIS` 时才有实际差别
- `[[curve]]` 与 `[[map]]` 是同一个模型的两个入口，维度数由 `dims` 决定而非由节名决定（写错节名不报错，维度不匹配才报）

## 5. 与数据源的合并

| 输入 | 轴关系来源 | 冲突处理 |
|---|---|---|
| ELF 加 A2L | A2L 权威 | 配置与 A2L 冲突 → 报错，不静默覆盖 |
| 仅 ELF | 配置是唯一来源 | 无对应配置 → 该数组按裸数组展示，不推导 |

A2L 档下配置仅用于覆盖显示名或补充 A2L 未描述的轴；轴长度一律以 A2L 的 `max_axis_points` 为准。

## 6. 校验规则

加载配置后必须全部通过，任一失败即拒绝加载并报出定位（文件、行、id）。

| 编号 | 规则 | 失败表现 |
|---|---|---|
| R1 | 轴 `len` 等于对应维度的 `dims[i]` | 报错 |
| R2 | `axes` 长度等于 `dims` 长度 | 报错 |
| R3 | 轴符号与值符号不得相同 | 报错 |
| R4 | `id` 唯一；引用必须存在 | 报错 |
| R5 | 多维数组无绑定 | 警告，按裸数组展示 |
| R6 | 同一轴被多个值引用 | 允许，不警告 |
| R7 | 轴符号必须存在且为可解析的数值类型 | 报错 |

R1 能挡住绝大多数顺序写错的情况，但方阵除外（见第 3 节）。

## 7. 导出到 A2L 的映射

| 本项目模型 | A2L 结构 |
|---|---|
| 轴 `kind = shared` | `AXIS_PTS` 对象 |
| 值引用 shared 轴 | `AXIS_DESCR COM_AXIS` + `AXIS_PTS_REF <轴名>` |
| 轴 `kind = inline` | `AXIS_DESCR STD_AXIS`（轴数据内嵌） |
| 一维值 | `CHARACTERISTIC` 类型 `CURVE` |
| 二维值 | `CHARACTERISTIC` 类型 `MAP` + `MATRIX_DIM` |
| `dims` 到 `MATRIX_DIM` | **逆序**：`dims = [A, B]` → `MATRIX_DIM B A` |

逆序的理由：C 的最后一维是内存中变化最快的维度，A2L 的 `MATRIX_DIM x y` 以 x 为快速维。这一条已在 M0-4 用真实的 `a2lfile` 写读回验证过（`AXIS_DESCR COM_AXIS` 加 `AXIS_PTS_REF` 完整保留）。

## 8. Golden 样例

输入取 M0-4 探针的夹具：`float cal_map[4][3]`、`cal_axis_x[3]`、`cal_axis_y[4]`（源码已入库：[fixtures/elf/calib.c](../fixtures/elf/calib.c)，编译方式见 [fixtures/README.md](../fixtures/README.md)）。

配置：

```toml
schema_version = 1

[[axis]]
id = "x_speed"
symbol = "cal_axis_x"

[[axis]]
id = "y_temp"
symbol = "cal_axis_y"

[[map]]
id = "eff_map"
symbol = "cal_map"
axes = ["y_temp", "x_speed"]
```

期望 IR 快照（地址取自 M0-4 实测，作为解析验证锚点；地址值本身不是约定内容）：

```json
{
  "axes": [
    { "id": "x_speed", "symbol": "cal_axis_x", "len": 3, "kind": "shared", "elem": "f32" },
    { "id": "y_temp", "symbol": "cal_axis_y", "len": 4, "kind": "shared", "elem": "f32" }
  ],
  "values": [
    {
      "id": "eff_map",
      "symbol": "cal_map",
      "kind": "map",
      "dims": [4, 3],
      "axes": ["y_temp", "x_speed"],
      "elem": "f32"
    }
  ],
  "diagnostics": []
}
```

对应的 A2L 导出片段（校验第 7 节的逆序规则）：

```
/begin AXIS_PTS x_speed "" 0x2040 NO_INPUT_QUANTITY RL 0 NO_COMPU_METHOD 3 -1.0E9 1.0E9
/end AXIS_PTS
/begin AXIS_PTS y_temp "" 0x2030 NO_INPUT_QUANTITY RL 0 NO_COMPU_METHOD 4 -1.0E9 1.0E9
/end AXIS_PTS
/begin CHARACTERISTIC eff_map "" MAP 0x2060 RL 0 NO_COMPU_METHOD -1.0E9 1.0E9
  MATRIX_DIM 3 4
  /begin AXIS_DESCR COM_AXIS NO_INPUT_QUANTITY NO_COMPU_METHOD 3 -1.0E9 1.0E9
    AXIS_PTS_REF x_speed
  /end AXIS_DESCR
  /begin AXIS_DESCR COM_AXIS NO_INPUT_QUANTITY NO_COMPU_METHOD 4 -1.0E9 1.0E9
    AXIS_PTS_REF y_temp
  /end AXIS_DESCR
/end CHARACTERISTIC
```

注意 `MATRIX_DIM 3 4` 与 `dims = [4, 3]` 是逆序关系，且两个 `AXIS_DESCR` 的顺序与 `axes` 顺序一致（x 在前）——A2L 内 `AXIS_DESCR` 的排列顺序按 x、y、z 递增，与 `axes` 的「内层优先」相反，这是最容易写反的一处，golden 必须锁死。

上面这段片段已用 `a2lfile` **严格模式**实测解析通过（`warnings=0`，读回 `type=Map`、`MATRIX_DIM [3, 4]`、`AXIS_PTS_REF` 依次为 `x_speed`、`y_temp`），不是纸面示意；片段全文在 [fixtures/a2l/axis-binding-golden.a2l](../fixtures/a2l/axis-binding-golden.a2l)。解析时需自备 `RECORD_LAYOUT RL` 与 `PROJECT`/`MODULE` 外壳——严格模式会因引用缺失而报错，这一点在 golden 落地时同样要遵守。

反向用例（必须报 R1 错）：`axes = ["x_speed", "y_temp"]` 时 `len=3` 对 `dims[0]=4` 不匹配。

## 9. 落地

- **M1**：`cal-core` 落轴与值的 IR 类型、TOML 解析、R1 至 R7 校验与单元测试；配置文件位置与解析入口在 M1 确定
- **M3**：golden 三件套入库（ELF 夹具、配置、期望 JSON 与 A2L 片段），档二 golden 的字节级对比以本文第 8 节为准
- 未决：三维及以上的暴露方式、轴排序与单调性是否进 M1 校验、本配置是否与 quirk 配置同文件——三项都留给 M1 决定，不影响当前格式（未知字段容忍已预留）
