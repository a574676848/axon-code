import test from 'node:test';
import assert from 'node:assert/strict';
import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';

/*
 * 后台注入轮（cron 回调、目标回调）的身份必须与索引骨架轮一致：
 * 帧里的用户消息 partId 是 `user:<action-id>`，若拿它当轮次标识，
 * 同一轮会先有一份骨架、再长出一份帧轮，正文里出现两条一样的用户消息。
 */

const INTERACTION = '01INJECTEDTURNTEST000000000';

function frame(partId, content, interactionId) {
  return {
    kind: 'frame',
    subscribeId: `interaction:${INTERACTION}`,
    eventType: 'wire_event',
    cursor: null,
    payloadJson: JSON.stringify({
      schemaVersion: 'axon.wire.v3',
      eventId: `${partId}:event`,
      correlationId: 'user',
      interactionId,
      session: { sessionId: 'session-injected' },
      sequence: 1,
      recordedAtMs: 1_000,
      kind: { type: 'userMessage', payload: { partId, content } },
    }),
  };
}

test('索引骨架与 durable 帧合并为一条用户消息', () => {
  let state = conversationReducer(INITIAL_CONVERSATION, {
    type: 'open-turn',
    turnId: INTERACTION,
    nowMs: 1_000,
  });
  state = conversationReducer(state, {
    type: 'frames',
    frames: [frame(`local-user:${INTERACTION}`, '后台 Goal 阶段结束', INTERACTION)],
    nowMs: 1_000,
  });
  state = conversationReducer(state, {
    type: 'frames',
    frames: [frame(`user:${INTERACTION}`, '后台 Goal 阶段结束', INTERACTION)],
    nowMs: 1_100,
  });

  assert.equal(state.turns.length, 1, '同一轮只能有一个轮次');
  assert.equal(
    state.turns[0].cells.filter((cell) => cell.role === 'user').length,
    1,
    '同一轮只能有一条用户消息',
  );
  assert.equal(state.turns[0].cells[0].text, '后台 Goal 阶段结束');
});

test('帧携带不同 Interaction 时各自成轮', () => {
  let state = conversationReducer(INITIAL_CONVERSATION, {
    type: 'open-turn',
    turnId: INTERACTION,
    nowMs: 1_000,
  });
  state = conversationReducer(state, {
    type: 'frames',
    frames: [frame('user:first', '第一轮', INTERACTION)],
    nowMs: 1_000,
  });
  // 第一轮收口之后，第二条用户消息才代表下一轮的开场。
  state = conversationReducer(state, {
    type: 'frames',
    frames: [
      {
        kind: 'frame',
        subscribeId: `interaction:${INTERACTION}`,
        eventType: 'turn_completed',
        cursor: null,
        payloadJson: JSON.stringify({ interactionId: INTERACTION, response: '好了', status: 'completed' }),
      },
    ],
    nowMs: 1_500,
  });
  state = conversationReducer(state, {
    type: 'frames',
    frames: [frame('user:second', '第二轮', '01ANOTHERINTERACTION00000000')],
    nowMs: 2_000,
  });

  assert.equal(state.turns.length, 2, '不同 Interaction 必须各自成轮');
});
