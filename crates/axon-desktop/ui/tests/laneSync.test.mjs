import test from 'node:test';
import assert from 'node:assert/strict';
import {
  EMPTY_LANE_SYNC_CURSOR,
  advanceLaneSync,
} from '../src/conversation/laneSync.ts';

test('首次见到一条车道只登记游标，不重复接管', () => {
  // 首次加载由目标切换本身完成，这里再触发一次就是两路同时读同一页。
  const step = advanceLaneSync(EMPTY_LANE_SYNC_CURSOR, 'lane-a', 2, false);
  assert.deepEqual(step.cursor, { laneId: 'lane-a', interactionCount: 2 });
  assert.equal(step.reattach, false);
});

test('车道轮数增长且本视图空闲时重新接管一次', () => {
  // 后台作业跑完由 daemon 唤醒出来的新一轮不在本视图的订阅里：必须重新接管，
  // 否则正文停在上一轮，状态位却已经按车道真实状态在走。
  const step = advanceLaneSync({ laneId: 'lane-a', interactionCount: 2 }, 'lane-a', 3, false);
  assert.equal(step.reattach, true);
  assert.deepEqual(step.cursor, { laneId: 'lane-a', interactionCount: 3 });
});

test('轮数没变不接管，避免每轮渲染都读一次历史', () => {
  const cursor = { laneId: 'lane-a', interactionCount: 3 };
  const step = advanceLaneSync(cursor, 'lane-a', 3, false);
  assert.equal(step.reattach, false);
  assert.deepEqual(step.cursor, cursor);
});

test('本视图正在跑时增长出来的那一轮是自己提交的，接管会让回放打在流式正文上', () => {
  const step = advanceLaneSync({ laneId: 'lane-a', interactionCount: 2 }, 'lane-a', 3, true);
  assert.equal(step.reattach, false);
  // 游标仍要推进：否则自己这一轮收口后会被当成外部新轮再接管一次。
  assert.deepEqual(step.cursor, { laneId: 'lane-a', interactionCount: 3 });
});

test('换车道重新登记，接管交给目标切换的首次加载', () => {
  const step = advanceLaneSync({ laneId: 'lane-a', interactionCount: 9 }, 'lane-b', 1, false);
  assert.equal(step.reattach, false);
  assert.deepEqual(step.cursor, { laneId: 'lane-b', interactionCount: 1 });
});

test('车道标识为空或计数不成立时不推进：那是「还不知道」而不是「没有变化」', () => {
  const cursor = { laneId: 'lane-a', interactionCount: 2 };
  for (const [laneId, count] of [
    ['   ', 5],
    ['lane-a', -1],
    ['lane-a', Number.NaN],
  ]) {
    const step = advanceLaneSync(cursor, laneId, count, false);
    assert.equal(step.reattach, false);
    assert.deepEqual(step.cursor, cursor);
  }
});
