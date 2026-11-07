// 样本数据源：实时流与离线文件走同一个入口。
//
// 曲线组件只认 [SampleSource]，不认传输。换假源演示、换文件重开（M5-05）
// 都只动构造那一行，取数与画图的代码零改动——这是 M5-01 的验收线。

import { OpenApeClient } from './generated/openape/v1/openape.client';
import type { DaqSample } from './generated/openape/v1/openape';

/** 一个采样点：自会话起点的微秒时间戳与解码后的值。 */
export interface SamplePoint {
  readonly tsUs: number;
  readonly value: number;
}

/** 数据源接口。离线源放完就结束，实时源由传输层断开而结束。 */
export interface SampleSource {
  points(): AsyncIterable<SamplePoint>;
}

/**
 * daemon 的 gRPC 实时流。
 *
 * DTO 载荷的布局知识（现在是 2 字节小端 u16）只活在这里：接口边界处就
 * 解码成物理值，曲线组件拿到的不再是要自己切的字节。换成多信号布局时
 * 也只改这一处。
 */
export class GrpcSampleSource implements SampleSource {
  private readonly client: OpenApeClient;

  constructor(client: OpenApeClient) {
    this.client = client;
  }

  async *points(): AsyncIterable<SamplePoint> {
    const call = this.client.streamSamples({});
    for await (const sample of call.responses) {
      yield decodeDto(sample);
    }
  }
}

/** DTO 载荷按小端 u16 解出采样值；载荷不足 2 字节时按 0 计。 */
function decodeDto(sample: DaqSample): SamplePoint {
  const view = new DataView(
    sample.payload.buffer as ArrayBuffer,
    sample.payload.byteOffset,
    sample.payload.byteLength,
  );
  const value = sample.payload.byteLength >= 2 ? view.getUint16(0, true) : 0;
  return { tsUs: Number(sample.timestampUs), value };
}

/**
 * daemon 的离线重开流（M5-05）：从一份 MF4 取指定通道的解值点。
 *
 * 与实时流走同一个 [SampleSource] 接口，所以离线曲线与实时曲线是同一条
 * 渲染路径（runCurve）；差别只在取数这一头。通道名不带时间主通道——
 * 时间轴是每份文件的 time 列，daemon 侧已经还原成微秒时间戳。
 */
export class GrpcRecordingSource implements SampleSource {
  private readonly client: OpenApeClient;
  private readonly path: string;
  private readonly channel: string;

  constructor(client: OpenApeClient, path: string, channel: string) {
    this.client = client;
    this.path = path;
    this.channel = channel;
  }

  async *points(): AsyncIterable<SamplePoint> {
    const call = this.client.openRecording({ path: this.path, channel: this.channel });
    for await (const point of call.responses) {
      yield { tsUs: Number(point.timestampUs), value: point.value };
    }
  }
}

/**
 * 给定点的回放源：假数据演示与离线重开的原型。
 *
 * 一次性放完全部点。要模拟实时节拍时在这里加间隔，别让曲线组件等。
 */
export class ArraySampleSource implements SampleSource {
  private readonly samples: readonly SamplePoint[];

  constructor(samples: readonly SamplePoint[]) {
    this.samples = samples;
  }

  async *points(): AsyncIterable<SamplePoint> {
    yield* this.samples;
  }
}
