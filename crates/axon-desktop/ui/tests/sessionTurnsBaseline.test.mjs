import test from 'node:test';
import assert from 'node:assert/strict';
import { baselineNeedsRefresh } from '../src/conversation/wire.ts';
import { handleTurnsFrame } from '../src/conversation/sessionTurns.ts';

/*
 * 轮次头基线对齐：订阅建立那一刻服务端已经知道的最新一轮。
 *
 * 「先读索引、后订阅」时，两次动作之间新建的那一轮会被折进基线，
 * interaction.created 永不播报；基线对不上就必须补读一次索引。
 */

const baseline = (interactionId) => ({
    latestInteractionId: interactionId,
    latestInteractionStatus: 'received',
    latestInteractionReceivedAtMs: 10,
});

test('基线指向已读到的轮次时不需要补读', () => {
    assert.equal(baselineNeedsRefresh(baseline('turn-2'), ['turn-1', 'turn-2']), false);
});

test('基线指向没读到的轮次时必须补读', () => {
    assert.equal(baselineNeedsRefresh(baseline('turn-3'), ['turn-1', 'turn-2']), true);
    // 索引还没落地：任何非空基线都值得补一次，重读按标识去重，多读一次不重排。
    assert.equal(baselineNeedsRefresh(baseline('turn-1'), []), true);
});

test('空会话没有基线，不做多余读取', () => {
    assert.equal(baselineNeedsRefresh(null, []), false);
    assert.equal(baselineNeedsRefresh(undefined, ['turn-1']), false);
    assert.equal(baselineNeedsRefresh({ ...baseline(''), latestInteractionId: '' }, []), false);
});
/* 轮次头帧的消费策略：认识的轮次不重读索引，只让不认识的触发一次。 */
const headFrame = (eventType, interactionId) => ({
    kind: 'frame',
    subscribeId: 'session-turns:s1',
    eventType,
    payloadJson: JSON.stringify({ interactionId }),
    cursor: null,
});

const hooks = () => {
    const calls = { refresh: 0, failure: 0 };
    return {
        calls,
        hooks: {
            requestRefresh: () => { calls.refresh += 1; },
            onFailure: () => { calls.failure += 1; },
        },
    };
};

test('已认识的轮次不再整页重读索引', () => {
    const spy = hooks();
    assert.equal(handleTurnsFrame(headFrame('interaction.created', 'turn-1'), ['turn-1'], spy.hooks), true);
    assert.equal(handleTurnsFrame(headFrame('interaction.settled', 'turn-1'), ['turn-1'], spy.hooks), true);
    assert.equal(spy.calls.refresh, 0, '自己的轮次正文与收口都由正文订阅推进');
});

test('外部新轮才触发一次索引重读', () => {
    const spy = hooks();
    handleTurnsFrame(headFrame('interaction.created', 'turn-9'), ['turn-1'], spy.hooks);
    assert.equal(spy.calls.refresh, 1);
});

test('订阅失败交给调用方重试，不冒充新轮', () => {
    const spy = hooks();
    assert.equal(
        handleTurnsFrame(
            { kind: 'failure', subscribeId: 'session-turns:s1', message: '订阅被拒绝', retryable: true },
            ['turn-1'],
            spy.hooks,
        ),
        true,
    );
    assert.equal(spy.calls.failure, 1);
    assert.equal(spy.calls.refresh, 0);
});

test('别的订阅标识不被轮次头消费', () => {
    const spy = hooks();
    assert.equal(
        handleTurnsFrame(
            { ...headFrame('interaction.created', 'turn-2'), subscribeId: 'interaction:i1' },
            [],
            spy.hooks,
        ),
        false,
    );
});
