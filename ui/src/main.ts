// 主页面：曲线 + 标定量表格 + 故障面板。
//
// 三条纪律在这里体现：地址一律相对（`location.origin`），不写死主机端口；
// 只支持根路径部署；故障只读 daemon 转发的 `diag::Fault`，界面不自己造
// 错误文案——否则界面上的故障与日志里的对不上，查不下去。

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
const variableBody = requireElement<HTMLTableSectionElement>('#variables');
const variableNote = requireElement<HTMLParagraphElement>('#variables-note');
const faultList = requireElement<HTMLUListElement>('#faults');

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

/**
 * 画标定量表格。
 *
 * `raw` 的那几列要标 `[raw]`：没有单位与换算时，界面上看到的就是内存里
 * 那几个字节，不填一个看着像单位的默认值。
 */
async function renderVariables(): Promise<void> {
  const { response: list } = await client.listVariables({});
  variableBody.replaceChildren();
  if (!list.loaded) {
    variableNote.textContent = '未加载描述文件：给 daemon 加 --elf 或 --a2l。';
    return;
  }
  for (const entry of list.variables) {
    const row = document.createElement('tr');

    const symbol = document.createElement('td');
    symbol.textContent = entry.symbol;
    row.append(symbol);

    const address = document.createElement('td');
    address.textContent = `0x${entry.address.toString(16).toUpperCase().padStart(8, '0')}`;
    row.append(address);

    const shape = document.createElement('td');
    const dims = entry.dims.length > 0 ? `[${entry.dims.join('][')}]` : '';
    shape.textContent = `${entry.elem}${dims}`;
    row.append(shape);

    const role = document.createElement('td');
    role.textContent = entry.role === '' ? '未指定' : entry.role;
    row.append(role);

    const semantics = document.createElement('td');
    semantics.textContent = entry.raw ? '[raw]' : '';
    semantics.className = entry.raw ? 'raw' : '';
    row.append(semantics);

    variableBody.append(row);
  }
  variableNote.textContent =
    list.notes.length > 0 ? `提示：${list.notes.join('；')}` : '';
}

/**
 * 画故障面板。
 *
 * 只把 daemon 给的字段摆出来，不自己判断严重度、不改写文案：故障的措辞
 * 与分级归 diag 管，界面另说一套就跟日志对不上了。
 */
async function renderFaults(): Promise<void> {
  const { response: list } = await client.listFaults({});
  faultList.replaceChildren();
  if (list.faults.length === 0) {
    const empty = document.createElement('li');
    empty.textContent = '暂无故障';
    faultList.append(empty);
    return;
  }
  for (const fault of list.faults) {
    const item = document.createElement('li');
    const head = document.createElement('strong');
    head.textContent = `[${fault.severity}] ${fault.code}`;
    item.append(head);
    item.append(document.createTextNode(` ${fault.message}`));
    if (fault.hint !== '') {
      const hint = document.createElement('em');
      hint.textContent = ` —— ${fault.hint}`;
      item.append(hint);
    }
    faultList.append(item);
  }
}

async function streamSamples(): Promise<void> {
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

async function main(): Promise<void> {
  // 表格与故障面板先画：它们不依赖样本流，曲线连不上时也该看得见。
  await renderVariables();
  await renderFaults();
  await streamSamples();
}

main().catch((err: unknown) => {
  statusLine.textContent = `出错了：${String(err)}`;
});
