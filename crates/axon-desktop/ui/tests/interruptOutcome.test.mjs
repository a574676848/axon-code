import test from 'node:test';
import assert from 'node:assert/strict';

import { interruptOutcome, readLatestTurnReading } from '../src/conversation/interruptOutcome.ts';

/*
 * 中断回执为假时的结论口径。
 *
 * 服务端说「没有活跃轮次」，索引却可能仍把这一轮记成在途：那正是「提示已经结束、
 * spinner 不消失」的现场。回执为假必须重读一次轮次索引再决定提示；索引仍说在途时，
 * 收口只能落在服务端（`settle-silent-turn`），界面不得自己把一轮标成已中断。
 */

/** 一条中断回执；默认是「没有活跃轮次」。 */
function receipt(overrides = {}) {
    return { sessionId: 'session-1', interrupted: false, cancellationRequested: false, ...overrides };
}

/** 旧后台没有收口接口：返回 null，界面如实说「没有可用的收口接口」。 */
const unsupportedSettle = async () => null;

test('interrupt_receipt_false_triggers_index_refetch：回执为假必须先重读轮次索引', async () => {
    let reads = 0;
    const outcome = await interruptOutcome(
        receipt(),
        async () => {
            reads += 1;
            return { kind: 'status', status: 'completed', interactionId: 'interaction-1' };
        },
        unsupportedSettle,
    );
    assert.equal(reads, 1, '回执为假时索引只重读一次');
    assert.equal(outcome.refetched, true);
    assert.equal(outcome.notice, '本轮已经结束，无需中断');
    assert.equal(outcome.settleRequested, false, '索引已收口时不得再请求服务端收口');
});

test('interrupt_settles_via_server_route_when_index_is_stale：索引仍说在途时收口落在服务端', async () => {
    const settled = [];
    const outcome = await interruptOutcome(
        receipt(),
        async () => ({ kind: 'status', status: 'processing', interactionId: 'interaction-live' }),
        async (interactionId) => {
            settled.push(interactionId);
            return { settled: true, verdict: 'lease_expired' };
        },
    );
    assert.deepEqual(settled, ['interaction-live'], '必须用索引里的那一条交互标识调服务端收口');
    assert.equal(outcome.settleRequested, true);
    assert.equal(outcome.verdict, 'lease_expired');
    assert.equal(
        outcome.notice,
        '已按服务端判定标记为已中断：这一轮的租约已经过期',
        '文案取服务端判据，界面不自己造结论',
    );

    // 服务端说仍在跑：如实转达，不宣称已经收口。
    const stillRunning = await interruptOutcome(
        receipt(),
        async () => ({ kind: 'status', status: 'processing', interactionId: 'interaction-live' }),
        async () => ({ settled: false, verdict: 'still_running' }),
    );
    assert.equal(stillRunning.notice, '服务端仍在跑这一轮，未做任何标记');
    assert.equal(stillRunning.settleRequested, true);
    assert.equal(stillRunning.verdict, 'still_running');

    // 旧后台没有这条路由：不假装收口成功。
    const unavailable = await interruptOutcome(
        receipt(),
        async () => ({ kind: 'status', status: 'processing', interactionId: 'interaction-live' }),
        unsupportedSettle,
    );
    assert.equal(unavailable.settleRequested, false);
    assert.equal(unavailable.notice.includes('收口接口'), true);
});

test('回执为真时不再读索引：受理与取消各有结论', async () => {
    let reads = 0;
    const read = async () => {
        reads += 1;
        return { kind: 'empty' };
    };
    assert.equal((await interruptOutcome(receipt({ interrupted: true }), read, unsupportedSettle)).notice, null);
    assert.equal(
        (
            await interruptOutcome(
                receipt({ interrupted: true, cancellationRequested: true }),
                read,
                unsupportedSettle,
            )
        ).notice,
        '已请求取消，本轮会在安全点收口',
    );
    assert.equal(reads, 0, '服务端已受理时索引只会比回执更旧，不重读');
});

test('索引读不出来时如实说未经复核，不冒充已确认', async () => {
    const outcome = await interruptOutcome(
        receipt(),
        async () => ({ kind: 'unreadable' }),
        unsupportedSettle,
    );
    assert.equal(outcome.notice, '本轮已经结束，无需中断（轮次索引未能复核）');
    assert.equal(outcome.refetched, true);
    assert.equal(outcome.settleRequested, false);
});

test('readLatestTurnReading：取最新一条，空页与读取失败分别落档', async () => {
    assert.deepEqual(
        await readLatestTurnReading(async () => [
            { status: 'completed', interactionId: 'a' },
            { status: 'processing', interactionId: 'b' },
        ]),
        { kind: 'status', status: 'processing', interactionId: 'b' },
        '最新一条在末尾，标识一并带回供服务端收口使用',
    );
    assert.deepEqual(await readLatestTurnReading(async () => []), { kind: 'empty' });
    assert.deepEqual(
        await readLatestTurnReading(async () => {
            throw new Error('网关不可达');
        }),
        { kind: 'unreadable' },
    );
});
