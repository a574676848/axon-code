import test from 'node:test';
import assert from 'node:assert/strict';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import { conversationReducer } from '../src/conversation/transcript/reducer.ts';

/*
 * 投影契约：同一串 durable 帧，无论一次全量喂入还是逐条增量喂入，最终都必须是同一份
 * 正文。实时流与历史回放共用这一条路径，因此这条断言是「两条投影路径重新分叉」的
 * 硬防线：任何一步只要依赖了到达批次而不是事件本身，它就会红。
 */

/** 一条 wire 帧。 */
function wire(sequence, kind, payload) {
    return {
        kind: 'frame',
        subscribeId: 'interaction:i1',
        eventType: 'wire_event',
        cursor: `cur-${sequence}`,
        payloadJson: JSON.stringify({
            schemaVersion: 'axon.wire.v3',
            eventId: `e${sequence}`,
            correlationId: 'user',
            session: { sessionId: 's1' },
            sequence,
            recordedAtMs: 1_000 + sequence,
            kind: { type: kind, payload },
        }),
    };
}

/** 一轮真实的 durable 帧序列：用户消息、思考、工具、正文、用量、终态。 */
function turnFrames() {
    return [
        wire(1, 'userMessage', { partId: 'u1', messageId: 'm1', content: '看下仓库根目录' }),
        wire(2, 'reasoningStart', { partId: 'r1', reasoningId: 'rid1' }),
        wire(3, 'reasoningDelta', { partId: 'r1', reasoningId: 'rid1', delta: '先看顶层目录' }),
        wire(4, 'reasoningEnd', { partId: 'r1', reasoningId: 'rid1' }),
        wire(5, 'toolCall', {
            partId: 't1',
            toolCallId: 'call1',
            toolName: 'read_dir',
            toolKind: 'read',
            summary: '列目录',
            argumentsSummary: '{"path":"."}',
        }),
        wire(6, 'toolResult', {
            partId: 't1',
            toolCallId: 'call1',
            status: 'completed',
            summary: '已列出',
            outputPreview: 'crates\ndocs',
        }),
        wire(7, 'assistantMessage', {
            partId: 'a1',
            messageId: 'm2',
            content: '第一个是 crates',
            delta: false,
        }),
        wire(8, 'usageUpdate', { inputTokens: 100, outputTokens: 5, cachedInputTokens: 40 }),
        {
            kind: 'frame',
            subscribeId: 'interaction:i1',
            eventType: 'turn_completed',
            cursor: 'cur-9',
            payloadJson: JSON.stringify({
                interactionId: 'i1',
                response: '第一个是 crates',
                status: 'completed',
            }),
        },
    ];
}

function opened() {
    return conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn',
        turnId: 'i1',
        nowMs: 990,
    });
}

test('全量喂入与逐条增量喂入得到同一份正文', () => {
    const frames = turnFrames();
    const bulk = conversationReducer(opened(), { type: 'frames', frames, nowMs: 9_000 });
    let incremental = opened();
    for (const frame of frames) {
        incremental = conversationReducer(incremental, {
            type: 'frames',
            frames: [frame],
            nowMs: 9_000,
        });
    }
    assert.deepEqual(incremental.turns, bulk.turns);
});

test('回放出来的一轮带着完整过程：思考、工具、正文与收尾读数', () => {
    const frames = turnFrames();
    const state = conversationReducer(opened(), { type: 'frames', frames, nowMs: 9_000 });
    const turn = state.turns[0];
    assert.deepEqual(
        turn.cells.map((cell) => cell.role),
        ['user', 'reasoning', 'tool', 'assistant', 'summary'],
    );
    assert.equal(turn.cells[1].text, '先看顶层目录');
    assert.equal(turn.cells[1].done, true);
    assert.equal(turn.cells[2].status, 'completed');
    assert.equal(turn.cells[2].output, 'crates\ndocs');
    assert.equal(turn.cells[3].text, '第一个是 crates');
    assert.equal(turn.status, 'completed');
});

test('分块喂入不改变思考正文：推理增量按事件追加而不是按批次重置', () => {
    const frames = turnFrames();
    // 把同一段推理拆成两条增量帧：无论一帧一条还是一批两条，正文都必须是拼接结果。
    const split = [
        ...frames.slice(0, 3),
        wire(10, 'reasoningDelta', { partId: 'r1', reasoningId: 'rid1', delta: '，再看依赖' }),
        ...frames.slice(3),
    ];
    const state = conversationReducer(opened(), {
        type: 'frames',
        frames: split,
        nowMs: 9_000,
    });
    const reasoning = state.turns[0].cells.find((cell) => cell.role === 'reasoning');
    assert.equal(reasoning.text, '先看顶层目录，再看依赖');
});

test('增量接管的轮次不会重复接进投影', () => {
    const first = conversationReducer(opened(), {
        type: 'frames',
        frames: turnFrames(),
        nowMs: 9_000,
    });
    const appended = conversationReducer(first, {
        type: 'append-turns',
        turns: first.turns,
    });
    assert.equal(appended, first, '同一轮重复接入必须原样返回');
});
