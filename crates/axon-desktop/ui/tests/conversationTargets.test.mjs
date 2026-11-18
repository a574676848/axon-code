import test from 'node:test';
import assert from 'node:assert/strict';
import { isStandaloneSession, targetOfSession } from '../src/conversation/targets.ts';
import { isReadOnlySession } from '../src/sessionOpening.ts';

const lane = {
  workspaceId: 'ws-1',
  workspacePath: 'E:/work',
  sessionId: 'session-session-abc',
  destination: 'terminal-1',
  title: '车道会话',
  channel: 'cli',
};

test('车道会话按 Interaction 流订阅，而不是按会话', () => {
  const target = targetOfSession(lane);
  assert.equal(target.kind, 'interaction');
  assert.equal(target.interactionId, lane.sessionId);
  assert.equal(target.destination, lane.destination);
});

test('后台驱动的独立会话按会话订阅', () => {
  const goal = targetOfSession({ ...lane, sessionId: 'run-1', channel: 'goal' });
  assert.equal(goal.kind, 'session');
  assert.equal(goal.sessionId, 'run-1');

  const cron = targetOfSession({ ...lane, sessionId: 'run-2', channel: 'cron' });
  assert.equal(cron.kind, 'session');

  // 子代理会话由渠道判定：会话标识是 ULID，不再靠标识里的一段字符认类型。
  const subagent = targetOfSession({
    ...lane,
    sessionId: '01J8Z000000000000000000AA',
    channel: 'subagent',
  });
  assert.equal(subagent.kind, 'session');
});

test('只读判定看渠道，车道会话仍可由用户提交', () => {
  assert.equal(isReadOnlySession(lane), false, '车道会话不是只读');
  assert.equal(isReadOnlySession({ ...lane, channel: 'goal' }), true);
  assert.equal(isReadOnlySession({ ...lane, channel: 'cron' }), true);
  assert.equal(isReadOnlySession(null), false);
  assert.equal(isStandaloneSession(lane), false);
});
