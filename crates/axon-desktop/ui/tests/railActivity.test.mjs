import test from 'node:test';
import assert from 'node:assert/strict';

import { railActivity } from '../src/components/railActivity.ts';

/*
 * 收轨态搜索格徽标与卡片的聚合口径。
 *
 * 徽标是折叠后唯一的状态提示，错一档就等于把「有结果未看」说成「一切安静」，
 * 所以这里逐档锁定：在跑的优先级、终态映射、待答不算结束、看过即清。
 */

const workspace = (sessions) => ({
  id: 'ws-1',
  name: '项目',
  path: '/repo',
  active: true,
  removable: false,
  sessions,
});

const session = (id, overrides = {}) => ({
  id,
  title: `会话 ${id}`,
  channel: 'desktop',
  destination: `dest-${id}`,
  audience: '',
  interaction_count: 0,
  created_at: 0,
  updated_at: 0,
  pinned: false,
  archived: false,
  pending: false,
  draft: false,
  running: false,
  ...overrides,
});

test('没有任何事实时不点徽标，也不生成卡片条目', () => {
  const result = railActivity([workspace([session('a')])], {}, null);
  assert.equal(result.cue, null);
  assert.deepEqual(result.sessions, []);
  assert.equal(result.questionCount, 0);
  assert.equal(result.runningCount, 0);
  assert.equal(result.unseenCount, 0);
});

test('完成终态点绿点，失败点红点，中断点琥珀点', () => {
  const list = [workspace([session('done'), session('failed'), session('stopped')])];
  const marks = { done: 'completed', failed: 'failed', stopped: 'interrupted' };
  const result = railActivity(list, marks, null);
  assert.equal(result.cue, 'error');
  assert.deepEqual(
    result.sessions.map((item) => [item.sessionId, item.cue]),
    [
      ['done', 'done'],
      ['failed', 'error'],
      ['stopped', 'warning'],
    ],
  );
  assert.equal(result.unseenCount, 3);
});

test('在跑高于任何未查看终态，停下后终态点才回来', () => {
  const list = [workspace([session('running', { running: true }), session('done')])];
  const marks = { running: 'completed', done: 'completed' };
  const busy = railActivity(list, marks, null);
  assert.equal(busy.cue, 'ongoing');
  assert.equal(busy.runningCount, 1);
  assert.equal(busy.unseenCount, 1);
  assert.deepEqual(
    busy.sessions.map((item) => item.sessionId),
    ['running', 'done'],
    '在跑排在未查看终态前面',
  );

  const quiet = railActivity([workspace([session('running'), session('done')])], marks, null);
  assert.equal(quiet.cue, 'done');
  assert.deepEqual(
    quiet.sessions.map((item) => item.sessionId).sort(),
    ['done', 'running'],
  );
});

test('实时轮次能把刚起、还没进快照的那一轮算进在跑', () => {
  const result = railActivity([workspace([session('fresh')])], {}, 'fresh');
  assert.equal(result.cue, 'ongoing');
  assert.equal(result.runningCount, 1);
  assert.deepEqual(
    result.sessions.map((item) => [item.sessionId, item.cue]),
    [['fresh', 'ongoing']],
  );
});

test('待答进卡片并点亮琥珀徽标，优先于在跑与终态', () => {
  const list = [workspace([
    session('asking', { running: true }),
    session('running', { running: true }),
    session('done'),
  ])];
  const marks = { asking: 'question_pending', done: 'completed' };
  const result = railActivity(list, marks, null);
  assert.equal(result.cue, 'question');
  assert.equal(result.questionCount, 1);
  assert.equal(result.runningCount, 1, '待答会话不与在跑合并');
  assert.equal(result.unseenCount, 1);
  assert.deepEqual(
    result.sessions.map((item) => [item.sessionId, item.cue]),
    [
      ['asking', 'question'],
      ['running', 'ongoing'],
      ['done', 'done'],
    ],
    '待答、在跑、终态依次排列',
  );
});

test('快照里的待答计数也算待答，即使还没有推送标识', () => {
  const result = railActivity(
    [workspace([session('asking', { pending_question_count: 2 })])],
    {},
    null,
  );
  assert.equal(result.cue, 'question');
  assert.equal(result.questionCount, 1);
  assert.deepEqual(
    result.sessions.map((item) => item.sessionId),
    ['asking'],
  );
});

test('看过即清：标识撤下后徽标与对应卡片条目一起消失', () => {
  const list = [workspace([session('a'), session('b')])];
  const before = railActivity(list, { a: 'completed', b: 'failed' }, null);
  assert.equal(before.cue, 'error');
  assert.equal(before.unseenCount, 2);

  const after = railActivity(list, { b: 'failed' }, null);
  assert.equal(after.cue, 'error');
  assert.deepEqual(
    after.sessions.map((item) => item.sessionId),
    ['b'],
  );

  const empty = railActivity(list, {}, null);
  assert.equal(empty.cue, null);
  assert.deepEqual(empty.sessions, []);
});

test('卡片条目带齐工作区与会话标识，供点击直接进详情', () => {
  const result = railActivity([workspace([session('target')])], { target: 'completed' }, null);
  assert.deepEqual(result.sessions, [
    { sessionId: 'target', workspaceId: 'ws-1', title: '会话 target', cue: 'done' },
  ]);
});
