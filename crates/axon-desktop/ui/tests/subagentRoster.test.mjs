import test from 'node:test';
import assert from 'node:assert/strict';
import {
  mergeSubagentRoster,
  reconciledSubagentStatus,
  shouldPollSubagentRoster,
} from '../src/conversation/subagentRoster.ts';

/** 转录里的一行：合并只读标识与展示字段，其余按缺省给。 */
const row = (agentId, overrides = {}) => ({
  agentId,
  label: agentId,
  status: 'completed',
  activity: '',
  elapsedMs: 0,
  lastSeenAtMs: 0,
  order: 0,
  usage: null,
  cacheHitRateBasisPoints: null,
  model: null,
  summary: null,
  events: [],
  ...overrides,
});

/** 名册里的一条：`agentId` 是唯一匹配键。 */
const entry = (agentId, overrides = {}) => ({
  sessionId: `run--${agentId}`,
  agentId,
  title: null,
  ...overrides,
});

test('名册里转录没有的子代理补成行，状态取末轮状态', () => {
  // 历史轮折叠后转录不再有这些行，入口仍要数得出来。
  const merged = mergeSubagentRoster(
    [row('subagent:/root/live-one')],
    [
      entry('subagent:/root/live-one'),
      entry('subagent:/root/history-one', { title: '历史子代理', lastTurnStatus: 'completed' }),
    ],
  );
  assert.equal(merged.length, 2);
  const added = merged.find((item) => item.agentId === 'subagent:/root/history-one');
  assert.equal(added.label, '历史子代理');
  assert.equal(added.status, 'completed');
  // 名册没有用量与耗时，因此两处都不占位，不伪造 0。
  assert.equal(added.usage, null);
  assert.equal(added.elapsedMs, 0);
});

test('同一个代理标识不会同时出现两行', () => {
  const merged = mergeSubagentRoster(
    [row('subagent:/root/review', { label: '转录里的名字' })],
    [entry('subagent:/root/review', { title: '名册里的名字' })],
  );
  assert.equal(merged.length, 1);
  // 转录行信息更全，不允许被名册覆盖。
  assert.equal(merged[0].label, '转录里的名字');
});

test('仍在跑的子代理按 running 补出来，失败与中断各归各的状态', () => {
  const merged = mergeSubagentRoster([], [
    entry('subagent:/root/a', { status: 'active', lastTurnStatus: 'active' }),
    entry('subagent:/root/b', { status: 'idle', lastTurnStatus: 'failed' }),
    entry('subagent:/root/c', { status: 'idle', lastTurnStatus: 'interrupted' }),
    entry('subagent:/root/d', { status: 'systemError' }),
  ]);
  const byId = new Map(merged.map((item) => [item.agentId, item.status]));
  assert.equal(byId.get('subagent:/root/a'), 'running');
  assert.equal(byId.get('subagent:/root/b'), 'failed');
  assert.equal(byId.get('subagent:/root/c'), 'interrupted');
  assert.equal(byId.get('subagent:/root/d'), 'failed');
});

test('没有对账结果时原样返回转录行，界面不会因此少一行', () => {
  const rows = [row('subagent:/root/one')];
  assert.deepEqual(mergeSubagentRoster(rows, null), rows);
  assert.deepEqual(mergeSubagentRoster(rows, []), rows);
});

test('名册轮询只在还会变的时候开：空闲且全部终态时零请求', () => {
  const done = [row('subagent:/root/one', { status: 'completed' })];
  assert.equal(shouldPollSubagentRoster(false, done), false);
  assert.equal(shouldPollSubagentRoster(true, done), true);
  for (const status of ['running', 'starting', 'waiting']) {
    assert.equal(shouldPollSubagentRoster(false, [row('subagent:/root/x', { status })]), true);
  }
});

test('这条车道一行子代理都没有时一次都不再拉', () => {
  assert.equal(shouldPollSubagentRoster(false, []), false);
  assert.equal(shouldPollSubagentRoster(true, []), false);
});

test('名册缺标题与模型时用代理标识兜底，不写空标题', () => {
  const merged = mergeSubagentRoster([], [entry('subagent:/root/nameless', { title: '   ', model: '' })]);
  assert.equal(merged[0].label, 'subagent:/root/nameless');
  assert.equal(merged[0].model, null);
});

test('名册缺代理标识的行直接跳过，不伪造标识', () => {
  const merged = mergeSubagentRoster([], [{ sessionId: 'run--x', agentId: '', title: '无标识' }]);
  assert.equal(merged.length, 0);
});

test('roster_active_does_not_override_settled_transcript：转录收口后名册 active 不再压住状态', () => {
  const running = row('subagent:/root/live-one', { status: 'running' });
  const activeIdle = { status: 'active', lastTurnStatus: 'active' };
  // 本视图还有在跑的轮次：名册说 active，沿用行内状态（它可能已经有更新的阶段）。
  assert.equal(reconciledSubagentStatus(running, activeIdle, false), 'running');
  // 转录已经收口：名册的 active 不得把行留在运行中，计数随之回落。
  assert.equal(reconciledSubagentStatus(running, activeIdle, true), 'completed');
  // 末轮给出终态时按终态收口，而不是一律算完成。
  assert.equal(
    reconciledSubagentStatus(running, { status: 'active', lastTurnStatus: 'failed' }, true),
    'failed',
  );
  assert.equal(
    reconciledSubagentStatus(running, { status: 'active', lastTurnStatus: 'interrupted' }, true),
    'interrupted',
  );
  // 名册已判 idle 时的纠正路径保持不变。
  assert.equal(
    reconciledSubagentStatus(running, { status: 'idle', lastTurnStatus: 'active' }, false),
    'completed',
  );
  // 行本身已是终态：没有任何需要纠正的东西。
  const done = row('subagent:/root/live-one', { status: 'completed' });
  assert.equal(reconciledSubagentStatus(done, activeIdle, false), 'completed');
  // 没对过账时原样沿用行内状态：没有第二份事实可比时纠正就是猜测。
  assert.equal(reconciledSubagentStatus(running, undefined, true), 'running');
  /*
   * 与 `RuntimeActions` 的运行中计数同源：入口上的 `N/M` 只数这三档，
   * 状态回落之后计数必须跟着落到零，否则入口会一直显示有一个子代理在跑。
   */
  const countedRunning = [running, done].filter((item) =>
    ['running', 'starting', 'waiting'].includes(reconciledSubagentStatus(item, activeIdle, true)),
  );
  assert.deepEqual(countedRunning, []);
});
