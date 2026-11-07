// 最小曲线页：订阅 daemon 的样本流，画最后 10 秒。
//
// 两条纪律在这里体现：地址一律相对（`location.origin`），不写死主机端口；
// 只支持根路径部署（vite config 的 base 是 `/`）。

import uPlot from 'uplot';
import 'uplot/dist/uPlot.min.css';

import { GrpcWebFetchTransport } from '@protobuf-ts/grpcweb-transport';

import { OpenApeClient } from './generated/openape/v1/openape.client';

/** 曲线最多保留的点数。 */
const MAX_POINTS = 600;

/** 横轴窗口（毫秒）。 */
const WINDOW_MS = 10_000;

const transport = new GrpcWebFetchTransport({ baseUrl: location.origin });
const client = new OpenApeClient(transport);

function requireElement<T extends Element>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) {
    throw new Error(`页面缺少 ${selector}`);
  }
  return element;
}

const statusLine = requireElement<HTMLParagraphElement>('#status');
const chartHost = requireElement<HTMLDivElement>('#chart');

const timestamps: number[] = [];
const values: number[] = [];

const plot = new uPlot(
  {
    width: chartHost.clientWidth || 800,
    height: 320,
    scales: { x: { time: false } },
    series: [{}, { label: 'sample', stroke: '#4c9aff' }],
  },
  [timestamps, values],
  chartHost,
);

function push(tsMs: number, value: number): void {
  timestamps.push(tsMs);
  values.push(value);

  const cutoff = tsMs - WINDOW_MS;
  let drop = 0;
  while (drop < timestamps.length && timestamps[drop] < cutoff) {
    drop += 1;
  }
  const overflow = Math.max(0, timestamps.length - MAX_POINTS);
  const remove = Math.max(drop, overflow);
  if (remove > 0) {
    timestamps.splice(0, remove);
    values.splice(0, remove);
  }
  plot.setData([timestamps, values]);
}

async function main(): Promise<void> {
  statusLine.textContent = '订阅样本流…';
  const call = client.streamSamples({});
  let count = 0;
  for await (const sample of call.responses) {
    // 采样信号是 2 字节，按小端 u16 解释；换成多信号后这里按布局切分。
    const view = new DataView(
      sample.payload.buffer as ArrayBuffer,
      sample.payload.byteOffset,
      sample.payload.byteLength,
    );
    const value = sample.payload.byteLength >= 2 ? view.getUint16(0, true) : 0;
    push(Number(sample.timestampUs) / 1000, value);
    count += 1;
    if (count % 10 === 1) {
      statusLine.textContent = `已收 ${count} 帧`;
    }
  }
  statusLine.textContent = `流结束，共 ${count} 帧`;
}

main().catch((err: unknown) => {
  statusLine.textContent = `出错了：${String(err)}`;
});
