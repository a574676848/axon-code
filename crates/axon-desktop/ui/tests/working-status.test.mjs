import test from 'node:test';
import assert from 'node:assert/strict';

import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import { workingStatusPrimary } from '../src/conversation/transcript/derive.ts';

/*
 * 工作状态行的口径：只写真实请求所在的供应商与阶段。
 *
 * 一个 provider 阶段事实都没有时，它必须写中性句，不能拿配置投影出来的路由顶替：
 * 渠道是运行时按池、亲和与健康选出来的，配置里的第一条常常不是真正被请求的那一家。
 * 这里用一条真实的四渠道失败序列钉住整条文案推进。
 */

function frame(type, payload) {
    const envelope = {
        schemaVersion: 'axon.wire.v3',
        eventId: `event:${type}`,
        correlationId: type === 'providerStatus' ? `provider:${payload.provider}:${payload.model}` : type,
        session: { sessionId: 'session-1' },
        sequence: 1,
        recordedAtMs: 1_000,
        kind: { type, payload },
    };
    return {
        kind: 'frame',
        subscribeId: 'interaction:external-interaction-1',
        eventType: 'wire_event',
        payloadJson: JSON.stringify(envelope),
        cursor: null,
    };
}

function openTurn() {
    return conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn',
        turnId: 'turn-1',
        nowMs: 1_000,
    });
}

/** 把一条帧喂进投影，返回这一轮此刻的状态行主文案。 */
function advance(state, type, payload) {
    const next = conversationReducer(state, {
        type: 'frames',
        frames: [frame(type, payload)],
        nowMs: 1_100,
    });
    const turn = next.turns[next.turns.length - 1];
    return { state: next, line: workingStatusPrimary(turn.activity ?? null) };
}

test('还没有供应商事实时写中性句，不伪造供应商', () => {
    const state = advance(openTurn(), 'userMessage', {
        partId: 'user:external-interaction-1',
        messageId: 'user-message:external-interaction-1',
        content: '继续',
        localFiles: [],
    });
    assert.equal(state.line, '正在连接供应商…');
    assert.ok(!state.line.includes('/'), '没有事实时不能出现任何供应商/模型名');
});

test('连接、重试、失败、换渠道与生成各写自己的阶段', () => {
    let state = openTurn();

    let step = advance(state, 'providerStatus', {
        provider: 'sensenova',
        model: 'sensenova-6.8-flash-lite',
        phase: 'connecting',
        transport: 'http',
        attemptIndex: 1,
    });
    state = step.state;
    assert.equal(step.line, 'sensenova/sensenova-6.8-flash-lite 正在连接');

    step = advance(state, 'providerStatus', {
        provider: 'sensenova',
        model: 'sensenova-6.8-flash-lite',
        phase: 'retrying',
        transport: 'http',
        attemptIndex: 2,
    });
    state = step.state;
    assert.equal(step.line, 'sensenova/sensenova-6.8-flash-lite 传输重试（第 2 次）');

    step = advance(state, 'providerStatus', {
        provider: 'sensenova',
        model: 'sensenova-6.8-flash-lite',
        phase: 'failed',
        transport: 'http',
        attemptIndex: 1,
        errorMessage: 'inference exceeds tpm/rpm limit',
    });
    state = step.state;
    assert.equal(
        step.line,
        'sensenova/sensenova-6.8-flash-lite 请求失败：inference exceeds tpm/rpm limit',
    );

    step = advance(state, 'providerStatus', {
        provider: 'sensenova',
        model: 'sensenova-6.8-flash-lite',
        phase: 'failover',
        transport: 'http',
        attemptIndex: 1,
        nextProvider: 'amd',
        nextModel: 'Qwen3.8-27B',
    });
    state = step.state;
    assert.equal(
        step.line,
        'sensenova/sensenova-6.8-flash-lite → amd/Qwen3.8-27B 正在切换渠道',
        '换渠道要同时写出前一家与后一家',
    );

    step = advance(state, 'providerStatus', {
        provider: 'amd',
        model: 'Qwen3.8-27B',
        phase: 'generating',
        transport: 'http',
        attemptIndex: 1,
    });
    assert.equal(step.line, 'amd/Qwen3.8-27B 生成中');
});

test('一条供应商流收口后仍保留真实渠道，不退回配置路由', () => {
    let state = openTurn();
    state = advance(state, 'providerStatus', {
        provider: 'amd',
        model: 'Qwen3.8-27B',
        phase: 'generating',
        transport: 'http',
        attemptIndex: 1,
    }).state;

    const settled = conversationReducer(state, {
        type: 'frames',
        frames: [frame('turnLifecycle', {
            partId: 'generation:stream:amd:Qwen3.8-27B:1',
            turnId: 'provider-stream',
            phase: 'regularTurn',
            status: 'completed',
            title: '生成',
            kind: 'StreamingCompleted',
            uiPhase: 'generation',
            attemptIndex: 1,
        })],
        nowMs: 1_200,
    });
    const turn = settled.turns[settled.turns.length - 1];
    assert.equal(turn.activity, 'amd/Qwen3.8-27B 生成中');
    assert.equal(workingStatusPrimary(turn.activity), 'amd/Qwen3.8-27B 生成中');
});
