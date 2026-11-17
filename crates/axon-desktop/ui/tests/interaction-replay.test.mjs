import test from 'node:test';
import assert from 'node:assert/strict';
import {
    HISTORY_PAGE_FRAMES,
    HISTORY_REPLAY_BUDGET,
    readInteractionFrames,
} from '../src/conversation/interactionReplay.ts';

/*
 * 历史帧的读取口径：整轮按页从头部推进到这一轮自己的 durable 尾，单轮超预算退化尾窗，
 * 代次作废时不把半截正文交出去。分页回放、展开与增量接管都走这一处。
 */

/** 一页历史帧。 */
function page(events, nextAfter, hasMore, tailCursor = null) {
    return { interactionId: 'i1', events, nextAfter, hasMore, tailSequence: 0, tailCursor };
}

function frame(eventType) {
    return { eventType, payloadJson: '{}', cursor: `c-${eventType}`, replay: true };
}

function harness(pages, options = {}) {
    const calls = [];
    let generation = 1;
    const fetchPage = async (interactionId, after, limit, tail) => {
        calls.push({ interactionId, after, limit, tail });
        if (options.afterFetch !== undefined) generation = options.afterFetch(generation, calls);
        const next = pages.shift();
        if (next === undefined) throw new Error('读取超出预期的页数');
        return next;
    };
    return { calls, fetchPage, generation: () => generation, bump: () => { generation += 1; } };
}

test('回放从头部逐页推进到这一轮自己的尾，不跳过思考与工具帧', async () => {
    const { calls, fetchPage, generation } = harness([
        page([frame('wire_event'), frame('wire_event')], 2, true, 'c2'),
        page([frame('wire_event'), frame('turn_completed')], 4, false, 'c4'),
    ]);
    const read = await readInteractionFrames('i1', 1, { generation, fetchPage });

    assert.deepEqual(calls, [
        { interactionId: 'i1', after: 0, limit: HISTORY_PAGE_FRAMES, tail: false },
        { interactionId: 'i1', after: 2, limit: HISTORY_PAGE_FRAMES, tail: false },
    ]);
    assert.equal(read.frames.length, 4, '两页的帧都要交出去');
    assert.deepEqual(
        read.frames.map((item) => item.subscribeId),
        Array(4).fill('interaction:i1'),
    );
    assert.equal(read.settled, true, '终态帧落在最后一页');
    assert.equal(read.cursor, 'c4', '订阅游标取这一轮真实尾帧');
});

test('这一轮还在跑时给出尾游标，由调用方接上实时增量', async () => {
    const { fetchPage, generation } = harness([page([frame('wire_event')], 1, false, 'c-last')]);
    const read = await readInteractionFrames('i1', 1, { generation, fetchPage });
    assert.equal(read.settled, false);
    assert.equal(read.cursor, 'c-last');
});

test('单轮帧数超过预算时退化成一次尾窗读取，不把读过的头部与尾窗叠着投两遍', async () => {
    const pages = [];
    for (let index = 0; index < HISTORY_REPLAY_BUDGET / HISTORY_PAGE_FRAMES; index += 1) {
        const frames = Array.from({ length: HISTORY_PAGE_FRAMES }, () => frame('wire_event'));
        pages.push(page(frames, index + 1, true, `head-${index}`));
    }
    pages.push(page([frame('wire_event'), frame('turn_completed')], 1, false, 'c-tail'));
    const { calls, fetchPage, generation } = harness(pages);
    const read = await readInteractionFrames('i1', 1, { generation, fetchPage });

    const tailCall = calls[calls.length - 1];
    assert.equal(tailCall.tail, true, '最后一页必须是尾窗');
    assert.equal(tailCall.after, 0);
    assert.equal(
        calls.filter((call) => call.tail === false).length,
        HISTORY_REPLAY_BUDGET / HISTORY_PAGE_FRAMES,
        '头部按预算读满即停',
    );
    assert.equal(read.frames.length, 2, '只有尾窗的帧被交出，头部那几页整体作废');
    assert.equal(read.cursor, 'c-tail');
});

test('代次不符时整次读取作废，不把半截正文交给新目标', async () => {
    const { fetchPage } = harness(
        [page([frame('wire_event')], 1, true, 'c1')],
        { afterFetch: (generation) => generation + 1 },
    );
    const read = await readInteractionFrames('i1', 1, { generation: () => 2, fetchPage });
    assert.equal(read, null);
});
