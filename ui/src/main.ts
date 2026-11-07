// 主页面：曲线 + 标定量表格 + 故障面板。
//
// 三条纪律在这里体现：地址一律相对（`location.origin`），不写死主机端口；
// 只支持根路径部署；故障只读 daemon 转发的 `diag::Fault`，界面不自己造
// 错误文案——否则界面上的故障与日志里的对不上，查不下去。

import uPlot from 'uplot';
import 'uplot/dist/uPlot.min.css';

import { GrpcWebFetchTransport } from '@protobuf-ts/grpcweb-transport';

import { OpenApeClient } from './generated/openape/v1/openape.client';

import { GrpcRecordingSource, GrpcSampleSource, SampleSource, type SamplePoint } from './sample-source';

/** 曲线最多保留的点数。 */
const MAX_POINTS = 600;

/** 横轴窗口（毫秒）。 */
const WINDOW_MS = 10_000;

/** 写入的审计署名。谁按了按钮，追溯时看这一栏。 */
const ACTOR = 'ui';

const transport = new GrpcWebFetchTransport({ baseUrl: location.origin });
const client = new OpenApeClient(transport);

/**
 * 曲线的数据源。换成假源（演示、测试）或离线文件源只动 import 与这一行，
 * 曲线组件零改动——数据源接口见 sample-source.ts。
 *
 * 离线重开（M5-05）：URL 带 `?replay=<MF4 路径>&channel=<通道>` 时换成
 * 文件回放源，渲染路径不变。
 */
const replayQuery = new URLSearchParams(location.search);
const replayPath = replayQuery.get('replay');
const dataSource: SampleSource = replayPath
  ? new GrpcRecordingSource(client, replayPath, replayQuery.get('channel') ?? '')
  : new GrpcSampleSource(client);

/** 实时只看最近窗口，离线要整段历史——push 的数据治理按这个分。 */
type CurveMode = 'live' | 'replay';

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
const writeResult = requireElement<HTMLParagraphElement>('#write-result');
const faultList = requireElement<HTMLUListElement>('#faults');
const calPageLabel = requireElement<HTMLElement>('#cal-page');
const diffNote = requireElement<HTMLParagraphElement>('#page-diff-note');
const diffBody = requireElement<HTMLTableSectionElement>('#page-diff-rows');

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

function push(point: SamplePoint, mode: CurveMode): void {
  const tsMs = point.tsUs / 1000;
  timestamps.push(tsMs);
  values.push(point.value);

  if (mode === 'replay') {
    // 离线要整段历史：不做时间窗口，只设总量上限防大文件卡死渲染。
    const overflow = Math.max(0, timestamps.length - MAX_POINTS);
    if (overflow > 0) {
      timestamps.splice(0, overflow);
      values.splice(0, overflow);
    }
  } else {
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
    address.textContent = hexAddress(entry.address);
    row.append(address);

    const shape = document.createElement('td');
    const dims = entry.dims.length > 0 ? `[${entry.dims.join('][')}]` : '';
    shape.textContent = `${entry.elem}${dims}`;
    row.append(shape);

    const role = document.createElement('td');
    role.textContent = entry.role === '' ? '未指定' : entry.role;
    row.append(role);

    const semantics = document.createElement('td');
    semantics.textContent = entry.raw ? '[raw]' : rangeText(entry.range);
    semantics.className = entry.raw ? 'raw' : '';
    row.append(semantics);

    row.append(writeCell(entry.symbol, entry.address, entry.elem));

    variableBody.append(row);
  }
  variableNote.textContent =
    list.notes.length > 0 ? `提示：${list.notes.join('；')}` : '';
}

function hexAddress(address: number): string {
  return `0x${address.toString(16).toUpperCase().padStart(8, '0')}`;
}

function rangeText(range: { min: number; max: number } | undefined): string {
  return range ? `量程 ${range.min}..=${range.max}` : '';
}

/**
 * 一行的写入控件。
 *
 * 写入前必须点确认：这是唯一会改变 ECU 状态的操作，误点一下代价是标定值
 * 被改掉。确认文案里带上符号与地址，避免"确认了一个看不见的对话框"。
 */
function writeCell(symbol: string, address: number, elem: string): HTMLTableCellElement {
  const cell = document.createElement('td');
  const input = document.createElement('input');
  input.type = 'text';
  input.size = 10;
  input.placeholder = elem === 'U8' ? 'hex' : 'value';
  const button = document.createElement('button');
  button.type = 'button';
  button.textContent = '写';
  button.addEventListener('click', () => {
    void submitWrite(symbol, address, input.value);
  });
  cell.append(input, button);
  return cell;
}

/**
 * 提交一次写入。
 *
 * 值为纯数字时按物理值送（daemon 按变量的元素类型与换算编码），否则按
 * 十六进制字节送。失败信息原样进故障面板——界面不改写 daemon 的措辞。
 */
async function submitWrite(symbol: string, address: number, raw: string): Promise<void> {
  const text = raw.trim();
  if (text === '') {
    writeResult.textContent = '给一个值再写：数字按物理值，其它按十六进制字节。';
    return;
  }
  if (!window.confirm(`写入 ${symbol} @ ${hexAddress(address)}？`)) {
    return;
  }
  const asNumber = Number(text);
  const isNumber = Number.isFinite(asNumber);
  const dataHex = isNumber ? '' : text.replace(/0x/gi, '').replace(/\s+/g, '');
  try {
    // 数字按物理值送：编码留在 daemon（cal_core::value 一份换算链），界面
    // 不重复实现字节序与换算。
    const { response } = await client.writeMemory({
      address: hexAddress(address),
      len: isNumber ? 0 : dataHex.length / 2,
      dataHex,
      symbol,
      actor: ACTOR,
      value: isNumber ? asNumber : undefined,
    });
    writeResult.textContent = [
      `${response.symbol} ${response.address}`,
      `写前 ${response.beforeHex}`,
      response.rangeChecked
        ? `量程 ${response.range?.min}..=${response.range?.max} 通过（物理值 ${response.range?.physical}）`
        : '[raw] 无量程信息，未校验',
      `写后 ${response.afterHex}`,
      `快照 ${response.snapshotPath}`,
    ].join(' · ');
  } catch (err) {
    reportFailure(err);
  }
}

/** 当前页。查询失败时不猜一个默认值。 */
async function refreshPage(): Promise<void> {
  try {
    const { response } = await client.getCalPage({});
    calPageLabel.textContent = String(response.page);
  } catch (err) {
    calPageLabel.textContent = '未知（daemon 未接管通道？）';
    reportFailure(err);
  }
}

async function setPage(page: number): Promise<void> {
  try {
    const { response } = await client.setCalPage({ page });
    calPageLabel.textContent = String(response.page);
  } catch (err) {
    reportFailure(err);
  }
}

/**
 * 页对比：只列差异地址。
 *
 * 一条差异都没有时明确写"两页一致"，与"没查"区分开。
 */
async function diffPages(reference: number, working: number): Promise<void> {
  diffBody.replaceChildren();
  diffNote.textContent = '';
  try {
    const { response } = await client.diffPages({
      referencePage: reference,
      workingPage: working,
    });
    if (response.differences.length === 0) {
      diffNote.textContent = `页 ${reference} 与页 ${working} 一致，无差异`;
      return;
    }
    diffNote.textContent = `页 ${reference} 与页 ${working} 差异 ${response.differences.length} 处`;
    for (const difference of response.differences) {
      const row = document.createElement('tr');
      for (const text of [
        difference.symbol,
        difference.address,
        difference.inReference ? difference.referenceHex : '—',
        difference.inWorking ? difference.workingHex : '—',
      ]) {
        const cell = document.createElement('td');
        cell.textContent = text;
        row.append(cell);
      }
      diffBody.append(row);
    }
  } catch (err) {
    diffNote.textContent = '页对比失败，原因见故障面板。';
    reportFailure(err);
  }
}

/**
 * 把失败摆到统一故障面板。
 *
 * 只搬运 daemon 或传输层给的文字，不另写一套措辞：界面上的故障必须能与
 * 日志里的同一条对上，否则查不下去。
 */
function reportFailure(err: unknown): void {
  const item = document.createElement('li');
  const head = document.createElement('strong');
  head.textContent = '[error] ui';
  item.append(head);
  item.append(document.createTextNode(` ${String(err)}`));
  faultList.prepend(item);
}

function wirePageControls(): void {
  requireElement<HTMLButtonElement>('#page-refresh').addEventListener('click', () => {
    void refreshPage();
  });
  requireElement<HTMLButtonElement>('#page-set-0').addEventListener('click', () => {
    void setPage(0);
  });
  requireElement<HTMLButtonElement>('#page-set-1').addEventListener('click', () => {
    void setPage(1);
  });
  requireElement<HTMLButtonElement>('#page-diff').addEventListener('click', () => {
    const reference = Number(requireElement<HTMLInputElement>('#page-reference').value);
    const working = Number(requireElement<HTMLInputElement>('#page-working').value);
    if (!Number.isInteger(reference) || !Number.isInteger(working)) {
      diffNote.textContent = '页号必须是整数。';
      return;
    }
    void diffPages(reference, working);
  });
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

async function runCurve(source: SampleSource, mode: CurveMode): Promise<void> {
  statusLine.textContent = replayPath
    ? `离线重开 ${replayPath}（${replayQuery.get('channel') ?? ''}）…`
    : '订阅样本流…';
  let count = 0;
  for await (const point of source.points()) {
    push(point, mode);
    count += 1;
    if (count % 10 === 1) {
      statusLine.textContent = `已收 ${count} 帧`;
    }
  }
  statusLine.textContent = `流结束，共 ${count} 帧`;
}

async function main(): Promise<void> {
  // 表格与故障面板先画：它们不依赖样本流，曲线连不上时也该看得见。
  wirePageControls();
  await renderVariables();
  await renderFaults();
  await refreshPage();
  await runCurve(dataSource, replayPath ? 'replay' : 'live');
}

main().catch((err: unknown) => {
  statusLine.textContent = `出错了：${String(err)}`;
});
