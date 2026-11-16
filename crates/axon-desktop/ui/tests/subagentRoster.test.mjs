import test from 'node:test';
import assert from 'node:assert/strict';
import { mergeSubagentRoster, shouldPollSubagentRoster, subagentSlug } from '../src/conversation/subagentRoster.ts';

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

/** 名册里的一条。 */
const entry = (slug, overrides = {}) => ({
  sessionId: `run--subagent--${slug}`,
  slug,
  ...overrides,
});

test('slug 与运行侧 child_session_id 的尾段同一口径', () => {
  assert.equal(subagentSlug('subagent:review/checker'), 'review_checker');
  assert.equal(subagentSlug('/lead'), 'lead');
  assert.equal(subagentSlug('subagent:simple'), 'simple');
});

test('名册里转录没有的子代理补成行，状态取末轮状态', () => {
  // 历史轮折叠后转录不再有这些行，入口仍要数得出来。
  const merged = mergeSubagentRoster(
    [row('subagent:live-one')],
    [entry('live-one'), entry('history-one', { title: '历史子代理', lastTurnStatus: 'completed' })],
  );
  assert.equal(merged.length, 2);
  const added = merged.find((item) => item.agentId === 'subagent:history-one');
  assert.equal(added.label, '历史子代理');
  assert.equal(added.status, 'completed');
  // 名册没有用量与耗时，因此两处都不占位，不伪造 0。
  assert.equal(added.usage, null);
  assert.equal(added.elapsedMs, 0);
});

test('同一个 slug 不会同时出现两行', () => {
  const merged = mergeSubagentRoster(
    [row('subagent:review/checker', { label: '转录里的名字' })],
    [entry('review_checker', { title: '名册里的名字' })],
  );
  assert.equal(merged.length, 1);
  // 转录行信息更全，不允许被名册覆盖。
  assert.equal(merged[0].label, '转录里的名字');
});

test('仍在跑的子代理按 running 补出来，失败与中断各归各的状态', () => {
  const merged = mergeSubagentRoster([], [
    entry('a', { status: 'active', lastTurnStatus: 'active' }),
    entry('b', { status: 'idle', lastTurnStatus: 'failed' }),
    entry('c', { status: 'idle', lastTurnStatus: 'interrupted' }),
    entry('d', { status: 'systemError' }),
  ]);
  const bySlug = new Map(merged.map((item) => [subagentSlug(item.agentId), item.status]));
  assert.equal(bySlug.get('a'), 'running');
  assert.equal(bySlug.get('b'), 'failed');
  assert.equal(bySlug.get('c'), 'interrupted');
  assert.equal(bySlug.get('d'), 'failed');
});

test('没有对账结果时原样返回转录行，界面不会因此少一行', () => {
  const rows = [row('subagent:one')];
  assert.deepEqual(mergeSubagentRoster(rows, null), rows);
  assert.deepEqual(mergeSubagentRoster(rows, []), rows);
});

test('名册轮询只在还会变的时候开：空闲且全部终态时零请求', () => {
  const done = [row('subagent:one', { status: 'completed' })];
  // 收口之后历史行是持久事实，再拉一次也不会变。
  assert.equal(shouldPollSubagentRoster(false, done), false);
  // 一轮在跑：子代理状态可能还在变。
  assert.equal(shouldPollSubagentRoster(true, done), true);
  // 轮次收口但行里还挂着未结束：继续拉到名册给出终态。
  for (const status of ['running', 'starting', 'waiting']) {
    assert.equal(shouldPollSubagentRoster(false, [row('subagent:x', { status })]), true);
  }
});

test('这条车道一行子代理都没有时一次都不再拉', () => {
  // 入口本来没有东西可画，拉回来的只会是空数组。
  assert.equal(shouldPollSubagentRoster(false, []), false);
  assert.equal(shouldPollSubagentRoster(true, []), false);
});

test('名册缺标题与模型时用 slug 兜底，不写空标题', () => {
  const merged = mergeSubagentRoster([], [entry('nameless', { title: '   ', model: '' })]);
  assert.equal(merged[0].label, 'nameless');
  assert.equal(merged[0].model, null);
});
