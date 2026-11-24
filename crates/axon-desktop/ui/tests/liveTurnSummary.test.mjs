import test from 'node:test';
import assert from 'node:assert/strict';
import { emptyTurn, applyTurnSummary, refreshLiveSummary } from '../src/conversation/transcript/turns.ts';
import { applyFrames } from '../src/conversation/transcript/frames.ts';
import { INITIAL_CONVERSATION, totalTokens } from '../src/conversation/transcript/state.ts';

/*
 * 分割线的出现时机与读数来源。
 *
 * 这里固定两件事：
 * 1. 生成中就有读数：用量帧一到，分割线立刻立起来，不必等整轮收口；
 * 2. 读数整段为零时不冒充一份有效读数：那一行只留用时，绝不写一排 Token 0。
 */

/** 一条 usageUpdate 帧：与事件通道同形，直接喂进归约器。 */
function usageFrame(turnId, payload, recordedAtMs) {
  return {
    kind: 'frame',
    subscribeId: `interaction:${turnId}`,
    eventType: 'wire_event',
    cursor: null,
    payloadJson: JSON.stringify({
      schemaVersion: 'axon.wire.v3',
      eventId: `${turnId}:${recordedAtMs}`,
      correlationId: 'test',
      session: { sessionId: turnId },
      sequence: recordedAtMs,
      recordedAtMs,
      kind: { type: 'usageUpdate', payload },
    }),
  };
}

/** 有一轮在跑、且已经产出正文的状态。 */
function runningState(turnId) {
  const opened = { ...INITIAL_CONVERSATION, turns: [emptyTurn(turnId, 1_000)] };
  return applyFrames(
    opened,
    [
      {
        kind: 'frame',
        subscribeId: `interaction:${turnId}`,
        eventType: 'wire_event',
        cursor: null,
        payloadJson: JSON.stringify({
          schemaVersion: 'axon.wire.v3',
          eventId: `${turnId}:assistant`,
          correlationId: 'test',
          session: { sessionId: turnId },
          sequence: 2,
          recordedAtMs: 1_500,
          kind: {
            type: 'assistantMessage',
            payload: { partId: `${turnId}:a`, messageId: `${turnId}:m`, content: '写了半句', delta: true },
          },
        }),
      },
    ],
    2_000,
  );
}

test('生成中收到用量帧就立起分割线，读数与耗时都是活的', () => {
  const turnId = 'interaction-live';
  const usage = {
    inputTokens: 120,
    cachedInputTokens: 900,
    cacheWriteInputTokens: 10,
    outputTokens: 40,
    reasoningOutputTokens: 5,
  };
  const state = applyFrames(
    runningState(turnId),
    [usageFrame(turnId, usage, 2_500)],
    2_600,
  );
  const turn = state.turns[0];
  const summary = turn.cells.find((cell) => cell.role === 'summary');
  assert.ok(summary !== undefined, '用量一到就该有分割线，而不是等收口');
  assert.equal(summary.usage?.outputTokens, 40, '输出 token 取实时帧的观测值');
  assert.ok(summary.durationMs !== null && summary.durationMs > 0, '耗时不写 0');
});

test('用量整段为零时不写一排零读数', () => {
  const turn = emptyTurn('interaction-empty', 1_000);
  const withAssistant = {
    ...turn,
    cells: [{ id: 'a', role: 'assistant', text: '只有正文', streaming: true, atMs: 1_100 }],
    index: { a: 0 },
  };
  const summarized = applyTurnSummary(withAssistant, {
    durationMs: 900,
    usage: null,
    firstTokenAvgMs: null,
    generationTpsX10: null,
  });
  const summary = summarized.cells.find((cell) => cell.role === 'summary');
  assert.equal(summary.usage, null, '没有观测就不带用量');
  assert.equal(summary.durationMs, 900, '用时照常给出');
});

test('收口后的轮次不再被实时刷新改写', () => {
  const turn = {
    ...emptyTurn('interaction-done', 1_000),
    status: 'completed',
    elapsedMs: 4_200,
    cells: [
      { id: 'a', role: 'assistant', text: '回答', streaming: false, atMs: 1_100 },
    ],
    index: { a: 0 },
    usage: {
      inputTokens: 10,
      cacheReadInputTokens: 0,
      cacheWriteInputTokens: 0,
      outputTokens: 5,
      reasoningOutputTokens: 0,
      currentContextTokens: 0,
      systemPromptTokens: 0,
      toolDefinitionTokens: 0,
      conversationTokens: 0,
    },
  };
  const refreshed = refreshLiveSummary(turn, 9_999);
  assert.equal(refreshed, turn, '收口轮次由收口路径定稿，实时刷新不再介入');
  assert.equal(totalTokens(refreshed.usage), 15);
});
