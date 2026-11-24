import test from 'node:test';
import assert from 'node:assert/strict';

import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';

/*
 * 终态帧的幂等口径。
 *
 * 收口帧会迟到、会重复、也会在重连后重放一遍。过去这些帧一律落到「最后一条未收口
 * 轮次」上，于是同一件事被写第二次：已经结算的用时按迟到时刻重算（实测把 29.9 秒
 * 写成 1 分 45 秒），已经中断的轮次被改写成完成，新一轮还会被上一轮的收尾帧提前收口。
 *
 * 现在三段结论各自钉住：
 * 1. 归属：载荷带 Interaction 标识时按它落帧，不再看「最后一条未收口」；
 * 2. 幂等：已收口轮次的状态与用时一个字都不改，只补可能缺失的分割线；
 * 3. 自动收口：服务端给出判据之后，界面按时收口并留一条可追溯的结论行。
 */

const NOW = 1_760_000_000_000;

/** 一轮已经收口的投影：状态完成，用时 30 秒，带一条分割线。 */
function settledTurn(id, { status = 'completed', elapsedMs = 30_000 } = {}) {
    return {
        id,
        interactionId: id,
        cells: [
            { id: `user:${id}`, role: 'user', text: '问题', atMs: NOW - 30_000, attachments: [] },
            { id: `assistant:${id}`, role: 'assistant', text: '回答', done: true },
            {
                id: `summary:${id}`,
                role: 'summary',
                durationMs: elapsedMs,
                usage: null,
                firstTokenAvgMs: null,
                generationTpsX10: null,
            },
        ],
        index: {},
        status,
        usage: {
            inputTokens: 0,
            cacheReadInputTokens: 0,
            cacheWriteInputTokens: 0,
            outputTokens: 0,
            reasoningOutputTokens: 0,
            currentContextTokens: 0,
        },
        usageFolded: {
            inputTokens: 0,
            cacheReadInputTokens: 0,
            cacheWriteInputTokens: 0,
            outputTokens: 0,
            reasoningOutputTokens: 0,
            currentContextTokens: 0,
        },
        usageStream: null,
        startedAtMs: NOW - 30_000,
        elapsedMs,
        firstEventAtMs: NOW - 30_000,
        lastEventAtMs: NOW,
        firstTokenAvgMs: null,
        generationTpsX10: null,
        generationOutputTokens: null,
        generationReasoningTokens: null,
        decodeMs: 0,
        activity: null,
        changes: [],
        subagents: [],
        pendingAttachments: [],
        pendingChannel: null,
        folded: false,
        expanded: false,
        summary: '',
        keepExpanded: false,
        settledAtMs: NOW,
    };
}

/** 一轮还在跑的投影：状态在流式，没有任何分割线。 */
function openTurn(id) {
    const turn = settledTurn(id);
    return {
        ...turn,
        cells: turn.cells.filter((cell) => cell.role !== 'summary'),
        status: 'streaming',
        elapsedMs: null,
        settledAtMs: null,
    };
}

/** 一条轮次完成帧：载荷带着归属标识。 */
function completedFrame(interactionId, { status = 'completed' } = {}) {
    return {
        kind: 'turn',
        subscribeId: `interaction:${interactionId}`,
        eventType: 'turn_completed',
        payloadJson: JSON.stringify({ interactionId, status, response: '回答' }),
        cursor: '1',
    };
}

/** 深冻结：任何写入都会抛错，用来证明「什么都不改」不是碰巧相等。 */
function frozen(state) {
    const clone = structuredClone(state);
    const walk = (value) => {
        if (value === null || typeof value !== 'object') return value;
        for (const item of Object.values(value)) walk(item);
        return Object.freeze(value);
    };
    return walk(clone);
}

test('late_completion_frame_does_not_rewrite_a_settled_turn', () => {
    const state = frozen({ ...INITIAL_CONVERSATION, turns: [settledTurn('turn-a')] });
    // 分割线中的用时是 30 秒；迟到帧带来的时刻是 75 秒之后。
    const next = conversationReducer(state, {
        type: 'frames',
        frames: [completedFrame('turn-a')],
        nowMs: NOW + 75_000,
    });
    const summary = next.turns[0].cells.find((cell) => cell.role === 'summary');
    assert.equal(summary.durationMs, 30_000, '用时保持首次结算的结果');
    assert.equal(next.turns[0].status, 'completed');
});

test('late_completion_frame_attributes_by_interaction_id', () => {
    // 上一轮已经收口，新一轮正在跑；上一轮的收尾帧迟到。
    const state = frozen({
        ...INITIAL_CONVERSATION,
        turns: [settledTurn('turn-a'), openTurn('turn-b')],
    });
    const next = conversationReducer(state, {
        type: 'frames',
        frames: [completedFrame('turn-a')],
        nowMs: NOW + 60_000,
    });
    assert.equal(next.turns[1].status, 'streaming', '新一轮不能被上一轮的收尾帧收口');
    assert.equal(next.turns[0].status, 'completed');
});

test('late_completion_frame_settles_the_turn_it_belongs_to', () => {
    const state = frozen({ ...INITIAL_CONVERSATION, turns: [openTurn('turn-a')] });
    const next = conversationReducer(state, {
        type: 'frames',
        frames: [completedFrame('turn-a')],
        nowMs: NOW,
    });
    assert.equal(next.turns[0].status, 'completed');
});

test('late_interaction_status_does_not_flip_an_interrupted_turn', () => {
    const interrupted = settledTurn('turn-a', { status: 'interrupted' });
    const state = frozen({ ...INITIAL_CONVERSATION, turns: [interrupted] });
    const next = conversationReducer(state, {
        type: 'frames',
        frames: [
            {
                kind: 'turn',
                subscribeId: 'interaction:turn-a',
                eventType: 'interaction.status',
                payloadJson: '"understood"',
                cursor: '1',
            },
        ],
        nowMs: NOW + 30_000,
    });
    assert.equal(next.turns[0].status, 'interrupted', '迟到状态帧不得把中断改回完成');
});

test('server_settled_turn_closes_the_round_and_leaves_a_conclusion', () => {
    const state = frozen({ ...INITIAL_CONVERSATION, turns: [openTurn('turn-a')] });
    const next = conversationReducer(state, {
        type: 'settle-silent-turn',
        interactionId: 'turn-a',
        verdict: 'no_in_flight_turn',
        nowMs: NOW + 5_000,
    });
    const turn = next.turns[0];
    assert.equal(turn.status, 'interrupted', '按服务端判据收口为中断');
    const notice = turn.cells.find((cell) => cell.role === 'notice');
    assert.equal(notice?.title, '已由服务端收口');
    assert.equal(
        notice?.summary.includes('早就结束'),
        true,
        '结论行说明服务端判据，不写界面自己的推测',
    );
    const summary = turn.cells.find((cell) => cell.role === 'summary');
    assert.notEqual(summary, undefined, '收口必须留下分割线，计时才会停');
});

test('server_settled_turn_is_idempotent', () => {
    const state = frozen({ ...INITIAL_CONVERSATION, turns: [settledTurn('turn-a')] });
    const next = conversationReducer(state, {
        type: 'settle-silent-turn',
        interactionId: 'turn-a',
        verdict: 'lease_expired',
        nowMs: NOW + 10_000,
    });
    assert.deepEqual(next.turns, state.turns, '已收口的轮次不再被服务端收口改写');
});
