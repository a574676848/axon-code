import test from 'node:test';
import assert from 'node:assert/strict';
import { baselineNeedsRefresh } from '../src/conversation/wire.ts';

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