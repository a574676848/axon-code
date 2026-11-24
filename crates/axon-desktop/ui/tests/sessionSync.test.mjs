import test from 'node:test';
import assert from 'node:assert/strict';
import {
  EMPTY_SESSION_SYNC_CURSOR,
  advanceSessionSync,
} from '../src/conversation/sessionSync.ts';

test('首次见到一条车道只登记游标，不重复接管', () => {
  // 首次加载由目标切换本身完成，这里再触发一次就是两路同时读同一页。
  const step = advanceSessionSync(EMPTY_SESSION_SYNC_CURSOR, 'session-a', 2, false);
  assert.deepEqual(step.cursor, { sessionId: 'session-a', interactionCount: 2 });
  assert.equal(step.reattach, false);
});

test('车道轮数增长且本视图空闲时重新接管一次', () => {
  // 后台作业跑完由 daemon 唤醒出来的新一轮不在本视图的订阅里：必须重新接管，
  // 否则正文停在上一轮，状态位却已经按车道真实状态在走。
  const step = advanceSessionSync({ sessionId: 'session-a', interactionCount: 2 }, 'session-a', 3, false);
  assert.equal(step.reattach, true);
  assert.deepEqual(step.cursor, { sessionId: 'session-a', interactionCount: 3 });
});

test('轮数没变不接管，避免每轮渲染都读一次历史', () => {
  const cursor = { sessionId: 'session-a', interactionCount: 3 };
  const step = advanceSessionSync(cursor, 'session-a', 3, false);
  assert.equal(step.reattach, false);
  assert.deepEqual(step.cursor, cursor);
});

test('本视图正在跑时增长出来的那一轮是自己提交的，接管会让回放打在流式正文上', () => {
  const step = advanceSessionSync({ sessionId: 'session-a', interactionCount: 2 }, 'session-a', 3, true);
  assert.equal(step.reattach, false);
  // 游标仍要推进：否则自己这一轮收口后会被当成外部新轮再接管一次。
  assert.deepEqual(step.cursor, { sessionId: 'session-a', interactionCount: 3 });
});

test('换车道重新登记，接管交给目标切换的首次加载', () => {
  const step = advanceSessionSync({ sessionId: 'session-a', interactionCount: 9 }, 'session-b', 1, false);
  assert.equal(step.reattach, false);
  assert.deepEqual(step.cursor, { sessionId: 'session-b', interactionCount: 1 });
});

test('车道标识为空或计数不成立时不推进：那是「还不知道」而不是「没有变化」', () => {
  const cursor = { sessionId: 'session-a', interactionCount: 2 };
  for (const [sessionId, count] of [
    ['   ', 5],
    ['session-a', -1],
    ['session-a', Number.NaN],
  ]) {
    const step = advanceSessionSync(cursor, sessionId, count, false);
    assert.equal(step.reattach, false);
    assert.deepEqual(step.cursor, cursor);
  }
});
