import test from 'node:test';
import assert from 'node:assert/strict';
import {
    interactionStillRunning,
    projectFrames,
} from '../src/conversation/historyProjection.ts';

/*
 * 回放的在跑判定：收口是服务端的事实，不是回放的默认值。
 *
 * 打开会话、增量接管与展开共用同一个投影。把还在流式输出的一轮判成「已完成」，
 * working 会当场变假：输入坞的工作状态行与中断入口消失、侧栏那一行的转圈也跟着
 * 没了，而正文其实还在长。
 */

/** 一条 wire 帧：与事件通道同形，直接喂进归约器。 */
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

/** 一轮还没收口的帧：用户消息、思考开始、思考增量，没有终态帧。 */
function runningFrames() {
    return [
        wire(1, 'userMessage', { partId: 'u1', messageId: 'm1', content: '看下仓库根目录' }),
        wire(2, 'reasoningStart', { partId: 'r1', reasoningId: 'rid1' }),
        wire(3, 'reasoningDelta', { partId: 'r1', reasoningId: 'rid1', delta: '先看顶层目录' }),
    ];
}

/** 同一轮收口之后的帧：接一条终态帧。 */
function completedFrames() {
    return [
        ...runningFrames(),
        {
            kind: 'frame',
            subscribeId: 'interaction:i1',
            eventType: 'turn_completed',
            cursor: 'cur-4',
            payloadJson: JSON.stringify({
                interactionId: 'i1',
                response: '第一个是 crates',
                status: 'completed',
            }),
        },
    ];
}

test('索引 lifecycle 的在跑判定只有两个取值域', () => {
    assert.equal(interactionStillRunning('received'), true);
    assert.equal(interactionStillRunning('processing'), true);
    for (const settled of ['understood', 'rejected', 'interrupted', 'failed', '']) {
        assert.equal(interactionStillRunning(settled), false, `${settled} 不是在跑状态`);
    }
});

test('在跑的一轮回放后仍是 streaming，不会被判成已完成', () => {
    const projected = projectFrames(
        'i1',
        runningFrames(),
        undefined,
        undefined,
        null,
        { settled: false },
    );

    assert.equal(projected.turn.status, 'streaming', '在跑的一轮必须保持 streaming');
    assert.equal(projected.turn.folded, false, '在跑的一轮不该被折叠');
    assert.ok(
        projected.turn.cells.some((cell) => cell.role === 'reasoning'),
        '过程正文照常长回来',
    );
});

test('确认收口的一轮照常写终态', () => {
    // 缺省按历史处理：帧里没有终态帧时也要收口，否则打开历史会话会永远转圈。
    const historical = projectFrames('i1', runningFrames(), undefined, undefined, null);
    assert.equal(historical.turn.status, 'completed');

    const closed = projectFrames(
        'i1',
        completedFrames(),
        undefined,
        undefined,
        null,
        { settled: true },
    );
    assert.equal(closed.turn.status, 'completed');

    // 帧读不到时调用方会拿空帧 + settled 收口：这条路径必须真的落成终态，
    // 否则骨架会挂着 streaming，工作状态行与侧栏转圈永远转下去。
    const unread = projectFrames('i1', [], undefined, undefined, null, { settled: true });
    assert.equal(unread.turn.status, 'completed');
});

test('帧自带的事件时刻是起点的权威事实', () => {
    const projected = projectFrames('i1', runningFrames(), undefined, undefined, null, {
        settled: false,
    });
    assert.equal(projected.turn.startedAtMs, 1_001, '起点取首条 durable 事件的记录时刻');
});

test('没有可用帧时，起跑时刻按索引用时还原', () => {
    const projected = projectFrames('i1', [], undefined, undefined, 5_000, { settled: false });
    assert.equal(projected.turn.status, 'streaming');
    const derived = Date.now() - projected.turn.startedAtMs;
    assert.ok(derived >= 4_000 && derived <= 7_500, `按索引用时还原起点，实测 ${derived}ms`);
});
