import test from 'node:test';
import assert from 'node:assert/strict';

import { applyFrames } from '../src/conversation/transcript/frames.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { emptyTurn } from '../src/conversation/transcript/turns.ts';

/*
 * 非主会话（子代理、cron、目标执行）的帧与主会话同形，唯一的历史差异是载荷里**没有**
 * `session` 块：归属只由帧的归属列给出，读边界再补 `interactionId`。
 *
 * 这条用例钉住投影侧的前提：轮次键只能取 `interactionId`，不能解引用 `session`——
 * 那样会在第一帧就抛错，整批帧被丢掉，详情页表现为「服务端没有返回任何帧」。
 */

const TURN = '01M80TF83F9CHB5ECQP1PR40JP';

/** 一帧子代理形状的 wire 事件：有归属标识，没有 session 块。 */
function frame(interactionId, kind, recordedAtMs, sequence) {
  return {
    kind: 'frame',
    subscribeId: `interaction:${interactionId}`,
    eventType: 'wire_event',
    cursor: null,
    payloadJson: JSON.stringify({
      schemaVersion: 'axon.wire.v3',
      eventId: `${interactionId}:${sequence}`,
      correlationId: 'agent-dispatch',
      interactionId,
      sequence,
      recordedAtMs,
      kind,
    }),
  };
}

test('subagent_frames_without_session_block_still_open_a_turn', () => {
  const state = applyFrames(
    INITIAL_CONVERSATION,
    [
      frame(TURN, { type: 'userMessage', payload: { partId: 'p-user', content: '想一想 1+1' } }, 1_000, 1),
      frame(
        TURN,
        {
          type: 'assistantMessage',
          payload: { partId: 'p-a', messageId: 'm-a', content: '因为…', delta: true },
        },
        1_200,
        2,
      ),
    ],
    1_300,
  );

  assert.equal(state.turns.length, 1, '没有 session 块的帧也必须开出一轮');
  assert.equal(state.turns[0].id, TURN, '轮次键取归属标识');
  assert.ok(
    state.turns[0].cells.some((cell) => cell.role === 'user'),
    '用户消息要落成单元',
  );
  assert.ok(
    state.turns[0].cells.some((cell) => cell.role === 'assistant' && cell.text.includes('因为')),
    '助手正文要落成单元',
  );
});

test('wire_frame_without_any_identity_is_ignored_not_crashed', () => {
  const payload = JSON.stringify({
    schemaVersion: 'axon.wire.v3',
    eventId: 'no-identity',
    correlationId: 'test',
    sequence: 1,
    recordedAtMs: 1_000,
    kind: { type: 'userMessage', payload: { partId: 'p', content: 'hi' } },
  });
  const state = applyFrames(
    INITIAL_CONVERSATION,
    [{ kind: 'frame', subscribeId: 'interaction:x', eventType: 'wire_event', cursor: null, payloadJson: payload }],
    1_100,
  );
  assert.equal(state.turns.length, 0, '既无归属也无 session 时不开轮，但不抛错');
});

test('replace_turn_inserts_a_turn_the_skeleton_filtered_out', () => {
    // 索引骨架会过滤「审计轮」，子代理会话唯一的一轮正是这种形状；按帧投影出来的整轮
    // 必须能被补进时间线，否则正文永远空白。
    const state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'replace-turn',
        turn: emptyTurn(TURN, 1_000),
        jobs: [],
    });
    assert.equal(state.turns.length, 1, '缺失的轮次必须补进来');
    assert.equal(state.turns[0].id, TURN);
});
