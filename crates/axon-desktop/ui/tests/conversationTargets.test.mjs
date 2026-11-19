import test from 'node:test';
import assert from 'node:assert/strict';
import { subscriptionIdOf, targetOfSession } from '../src/conversation/targets.ts';
import { sessionFactsOf } from '../src/conversation/sessionFacts.ts';
import { sessionWriteGate } from '../src/sessionOpening.ts';

const lane = {
  workspaceId: 'ws-1',
  workspacePath: 'E:/work',
  sessionId: 'session-session-abc',
  destination: 'terminal-1',
  title: '会话',
  channel: 'cli',
};

const goalFacts = {
  kind: 'goal',
  origin: 'goal',
  writable: false,
  canInterrupt: true,
  canAnswerQuestions: false,
  title: '目标执行',
  provider: 'sensenova',
  model: 'deepseek-flash',
  modelName: 'fast_model',
  interactionCount: 2,
  latestInteractionId: 'interaction-2',
};

test('所有会话都按 Interaction 目标订阅，后台会话没有第二套订阅', () => {
  for (const channel of ['cli', 'desktop', 'goal', 'cron', 'subagent']) {
    const target = targetOfSession({ ...lane, channel, sessionId: 'run-1' });
    assert.equal(target.kind, 'interaction');
    assert.equal(target.interactionId, 'run-1');
    assert.equal(target.workspaceId, lane.workspaceId);
    assert.equal(subscriptionIdOf(target), 'interaction:run-1');
  }
});

test('草稿会话也走 Interaction 目标，但带上 draft 标记', () => {
  const target = targetOfSession({ ...lane, sessionId: 'draft-1', draft: true });
  assert.equal(target.kind, 'interaction');
  assert.equal(target.draft, true);
});

test('能力块解析出可写性与能力位', () => {
  const facts = sessionFactsOf(goalFacts);
  assert.equal(facts?.writable, false);
  assert.equal(facts?.canInterrupt, true);
  assert.equal(facts?.canAnswerQuestions, false);
  assert.equal(facts?.latestInteractionId, 'interaction-2');
});

test('能力块缺失或为 null 时不给结论', () => {
  assert.equal(sessionFactsOf(null), null);
  assert.equal(sessionFactsOf({}), null);
  assert.equal(sessionFactsOf({ kind: 'primary', writable: true })?.writable, true);
});

test('写入口只认能力事实：未就绪不放开，只读与作答各按能力位', () => {
  // 首屏与草稿没有服务端事实，天然就绪且可写。
  assert.deepEqual(sessionWriteGate(null, null), {
    ready: true,
    readOnly: false,
    answerable: true,
  });
  assert.equal(sessionWriteGate({ draft: true }, null).ready, true);

  // 真实会话在能力事实到达前一律不可写。
  assert.equal(sessionWriteGate({ draft: false }, null).ready, false);

  // 后台驱动的会话：只读且不可答问。
  const goal = sessionWriteGate({ draft: false }, sessionFactsOf(goalFacts));
  assert.equal(goal.ready, true);
  assert.equal(goal.readOnly, true);
  assert.equal(goal.answerable, false);

  // 主会话：可写、可答问。
  const primary = sessionWriteGate(
    { draft: false },
    sessionFactsOf({ kind: 'primary', writable: true, canAnswerQuestions: true }),
  );
  assert.equal(primary.readOnly, false);
  assert.equal(primary.answerable, true);
});
