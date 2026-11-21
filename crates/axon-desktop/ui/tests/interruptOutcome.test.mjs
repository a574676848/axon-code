import test from 'node:test';
import assert from 'node:assert/strict';

import { interruptOutcome, readLatestTurnReading } from '../src/conversation/interruptOutcome.ts';

/*
 * 中断回执为假时的结论口径。
 *
 * 服务端说「没有活跃轮次」，索引却可能仍把这一轮记成在途：那正是「提示已经结束、
 * spinner 不消失」的现场。回执为假必须重读一次轮次索引再决定提示，两种结论都要
 * 说出来，不能静默保持转圈，也不能单方面把一轮标成已中断。
 */

/** 一条中断回执；默认是「没有活跃轮次」。 */
function receipt(overrides = {}) {
    return { sessionId: 'session-1', interrupted: false, cancellationRequested: false, ...overrides };
}

test('interrupt_receipt_false_triggers_index_refetch：回执为假必须先重读轮次索引', async () => {
    let reads = 0;
    const outcome = await interruptOutcome(receipt(), async () => {
        reads += 1;
        return { kind: 'status', status: 'completed' };
    });
    assert.equal(reads, 1, '回执为假时索引只重读一次');
    assert.equal(outcome.refetched, true);
    assert.equal(outcome.notice, '本轮已经结束，无需中断');
    assert.equal(outcome.notice.includes('待修复'), false, '索引与回执一致时不给修复提示');
});

test('索引仍说在途时如实提示界面状态待修复', async () => {
    const outcome = await interruptOutcome(receipt(), async () => ({
        kind: 'status',
        status: 'processing',
    }));
    assert.equal(outcome.notice, '服务端无运行轮次，界面状态待修复');
});

test('回执为真时不再读索引：受理与取消各有结论', async () => {
    let reads = 0;
    const read = async () => {
        reads += 1;
        return { kind: 'empty' };
    };
    assert.equal((await interruptOutcome(receipt({ interrupted: true }), read)).notice, null);
    assert.equal(
        (await interruptOutcome(receipt({ interrupted: true, cancellationRequested: true }), read))
            .notice,
        '已请求取消，本轮会在安全点收口',
    );
    assert.equal(reads, 0, '服务端已受理时索引只会比回执更旧，不重读');
});

test('索引读不出来时如实说未经复核，不冒充已确认', async () => {
    const outcome = await interruptOutcome(receipt(), async () => ({ kind: 'unreadable' }));
    assert.equal(outcome.notice, '本轮已经结束，无需中断（轮次索引未能复核）');
    assert.equal(outcome.refetched, true);
});

test('readLatestTurnReading：取最新一条，空页与读取失败分别落档', async () => {
    assert.deepEqual(
        await readLatestTurnReading(async () => [{ status: 'completed' }, { status: 'processing' }]),
        { kind: 'status', status: 'processing' },
        '最新一条在末尾',
    );
    assert.deepEqual(await readLatestTurnReading(async () => []), { kind: 'empty' });
    assert.deepEqual(
        await readLatestTurnReading(async () => {
            throw new Error('网关不可达');
        }),
        { kind: 'unreadable' },
    );
});
