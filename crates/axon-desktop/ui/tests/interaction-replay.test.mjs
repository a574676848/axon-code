import test from 'node:test';
import assert from 'node:assert/strict';
import {
    HISTORY_PAGE_FRAMES,
    HISTORY_REPLAY_PAGES,
    compactUsageSnapshots,
    readInteractionFrames,
} from '../src/conversation/interactionReplay.ts';

/*
 * 历史帧的读取口径：整轮按页从头部推进到这一轮自己的 durable 尾；用量快照按流压缩成
 * 最后一帧；页数用尽时保留「首页 + 尾窗」并如实标记中段被略过；代次作废时不把半截
 * 正文交出去。分页回放、展开与增量接管都走这一处。
 */

/** 一页历史帧。 */
function page(events, nextAfter, hasMore, tailCursor = null) {
    return { interactionId: 'i1', events, nextAfter, hasMore, tailSequence: 0, tailCursor };
}

function frame(eventType, cursor) {
    return { eventType, payloadJson: '{}', cursor: cursor ?? `c-${eventType}`, replay: true };
}

/** 一条用量快照帧：载荷里带用量类型名，读取层据此识别并压缩。 */
function usageFrame(cursor, outputTokens) {
    return {
        eventType: 'wire_event',
        payloadJson: `{"kind":{"payload":{"outputTokens":${outputTokens}},"type":"usageUpdate"}}`,
        cursor,
        replay: true,
    };
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
        page([frame('wire_event', 'c1'), frame('wire_event', 'c2')], 2, true, 'c2'),
        page([frame('wire_event', 'c3'), frame('turn_completed', 'c4')], 4, false, 'c4'),
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
    assert.equal(read.truncated, false, '整轮读完不算截断');
});

test('这一轮还在跑时给出尾游标，由调用方接上实时增量', async () => {
    const { fetchPage, generation } = harness([page([frame('wire_event', 'c-last')], 1, false, 'c-last')]);
    const read = await readInteractionFrames('i1', 1, { generation, fetchPage });
    assert.equal(read.settled, false);
    assert.equal(read.cursor, 'c-last');
});

test('用量快照按流压缩成最后一帧，其余帧一帧不动', () => {
    const frames = [
        frame('wire_event', 'a'),
        usageFrame('u1', 10),
        usageFrame('u2', 20),
        usageFrame('u3', 30),
        frame('wire_event', 'b'),
        usageFrame('u4', 40),
    ].map((item) => ({ kind: 'frame', subscribeId: 'interaction:i1', ...item }));

    const compacted = compactUsageSnapshots(frames);
    assert.deepEqual(
        compacted.map((item) => item.cursor),
        ['a', 'u3', 'b', 'u4'],
        '每段连续快照只留最后一条，其余帧顺序与数量不变',
    );
});

test('这一轮还在跑时同样压缩快照，界面拿到的仍是同一条流的最终读数', async () => {
    const { fetchPage, generation } = harness([
        page([usageFrame('u1', 1), usageFrame('u2', 2)], 2, false, 'c2'),
    ]);
    const read = await readInteractionFrames('i1', 1, { generation, fetchPage });
    assert.equal(read.frames.length, 1);
    assert.equal(read.frames[0].cursor, 'u2');
});

test('单轮页数超过上限时保留首页与尾窗，绝不把开头整段丢掉', async () => {
    const pages = [];
    for (let index = 0; index < HISTORY_REPLAY_PAGES; index += 1) {
        const frames = [
            frame('wire_event', `head-${index}-0`),
            frame('wire_event', `head-${index}-1`),
        ];
        pages.push(page(frames, index + 1, true, `head-${index}`));
    }
    pages.push(page([frame('wire_event', 'tail-0'), frame('turn_completed', 'tail-1')], 1, false, 'c-tail'));
    const { calls, fetchPage, generation } = harness(pages);
    const read = await readInteractionFrames('i1', 1, { generation, fetchPage });

    const tailCall = calls[calls.length - 1];
    assert.equal(tailCall.tail, true, '最后一页必须是尾窗');
    assert.equal(tailCall.after, 0);
    assert.equal(
        calls.filter((call) => call.tail === false).length,
        HISTORY_REPLAY_PAGES,
        '头部按页数上限读满即停',
    );
    assert.deepEqual(
        read.frames.map((item) => item.cursor),
        ['head-0-0', 'head-0-1', 'tail-0', 'tail-1'],
        '首页的抬头与尾窗的结论都要交出去，中段如实略过',
    );
    assert.equal(read.truncated, true, '中段被略过必须如实标记');
    assert.equal(read.settled, true);
    assert.equal(read.cursor, 'c-tail');
});

test('首页与尾窗重叠时按游标去重，同一帧不投两遍', async () => {
    const pages = [];
    for (let index = 0; index < HISTORY_REPLAY_PAGES; index += 1) {
        pages.push(page([frame('wire_event', `dup-${index}`)], index + 1, true, `dup-${index}`));
    }
    // 尾窗把首页那一帧又取回来一次：实际发生过，窄轮次里两段本来就相交。
    pages.push(page([frame('wire_event', 'dup-0'), frame('wire_event', 'dup-tail')], 1, false, 'c-tail'));
    const { fetchPage, generation } = harness(pages);
    const read = await readInteractionFrames('i1', 1, { generation, fetchPage });

    assert.deepEqual(
        read.frames.map((item) => item.cursor),
        ['dup-0', 'dup-tail'],
        '重复的帧只留一份',
    );
});

test('代次不符时整次读取作废，不把半截正文交给新目标', async () => {
    const { fetchPage } = harness(
        [page([frame('wire_event', 'c1')], 1, true, 'c1')],
        { afterFetch: (generation) => generation + 1 },
    );
    const read = await readInteractionFrames('i1', 1, { generation: () => 2, fetchPage });
    assert.equal(read, null);
});
