"""第三方互测（M5-02）：用 asammdf 读 golden 样本，核对通道与样本。

mf4-rs 既当写出底座又当读回验收工具，同库互证不出对方的错，所以
第三方参照选 asammdf（BSD-3，独立实现）。它在 fixtures/mf4/
golden.minimal.mf4 上逐条核对时间轴与两路取值。

用法：uv run --no-project --with asammdf python script/verify-mf4.py
"""

from pathlib import Path

from asammdf import MDF

GOLDEN = Path(__file__).resolve().parent.parent / "fixtures" / "mf4" / "golden.minimal.mf4"

TIMES = [0.0, 0.1, 0.2, 0.3, 0.4]
SPEED = [10.0, 20.5, -3.25, 0.0, 99.0]
THROTTLE = [0.0, 12.5, 45.0, 8.0, 100.0]


def main() -> None:
    mdf = MDF(str(GOLDEN))
    time = mdf.get("time")
    speed = mdf.get("engine_speed")
    throttle = mdf.get("throttle")

    assert list(time.timestamps) == TIMES, f"时间轴不一致：{list(time.timestamps)}"
    assert list(speed.samples) == SPEED, f"engine_speed 不一致：{list(speed.samples)}"
    assert list(throttle.samples) == THROTTLE, f"throttle 不一致：{list(throttle.samples)}"
    print(f"asammdf 读回一致：MDF {mdf.version}，{len(mdf.groups)} 组，5 条样本")


if __name__ == "__main__":
    main()
