import test from 'node:test';
import assert from 'node:assert/strict';
import {
    HISTORY_PAGE_FRAMES,
    REPLAY_MAX_PAGES,
    SEQUENTIAL_REPLAY_PAGES,
    compactUsageSnapshots,
    cursorSequence,
    readInteractionFrames,
    readInteractionFramesTailFirst,
    replayPageBudget,
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
    for (let index = 0; index < SEQUENTIAL_REPLAY_PAGES; index += 1) {
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
        SEQUENTIAL_REPLAY_PAGES,
        '头部按页数上限读满即停',
    );
    assert.equal(read.truncated, true, '中段被略过必须如实标记');
    assert.equal(read.settled, true);
    assert.equal(read.cursor, 'c-tail');
});

test('首页与尾窗重叠时按游标去重，同一帧不投两遍', async () => {
    const pages = [];
    for (let index = 0; index < SEQUENTIAL_REPLAY_PAGES; index += 1) {
        pages.push(page([frame('wire_event', `dup-${index}`)], index + 1, true, `dup-${index}`));
    }
    // 尾窗把首页那一帧又取回来一次：实际发生过，窄轮次里两段本来就相交。
    pages.push(page([frame('wire_event', 'dup-0'), frame('wire_event', 'dup-tail')], 1, false, 'c-tail'));
    const { fetchPage, generation } = harness(pages);
    const read = await readInteractionFrames('i1', 1, { generation, fetchPage });

    assert.deepEqual(
        read.frames.map((item) => item.cursor),
        ['dup-0', 'dup-1', 'dup-2', 'dup-3', 'dup-4', 'dup-tail'],
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

/* ------------------------------------------------------------------------- */
/* 尾窗先行：首屏先出尾段，前段在后台补齐后整轮重投。                          */
/* ------------------------------------------------------------------------- */

/** 真实形状的 durable 游标：补前段要靠序号判断读到哪一段为止。 */
function cursor(sequence) {
    return JSON.stringify({ generation: 'g', sequence });
}

/** 拆成两段句柄的读法：等尾窗出屏，再等补齐结果。 */
function beginTailFirst(pages, options = {}) {
    const harnessed = harness(pages, options);
    let painted = null;
    const reading = readInteractionFramesTailFirst('i1', 1, {
        generation: harnessed.generation,
        fetchPage: harnessed.fetchPage,
        ...options,
        onTail: (tail) => {
            painted = tail;
        },
    });
    return { ...harnessed, reading, painted: () => painted };
}

test('尾窗先行：一次往返先出屏，再按页补前段并整轮重投', async () => {
    const test$ = beginTailFirst(
        [
            // 尾窗：满一页（limit=2），这一轮的最后两帧在序号 90、91。
            page([frame('wire_event', cursor(90)), frame('turn_completed', cursor(91))], 91, false, cursor(91)),
            // 前段第 1 页：序号 1、50。
            page([frame('wire_event', cursor(1)), frame('wire_event', cursor(50))], 50, true, cursor(50)),
            // 前段第 2 页：51 与 90，越过尾窗起点。
            page([frame('wire_event', cursor(51)), frame('wire_event', cursor(90))], 90, false, cursor(90)),
        ],
        { limit: 2 },
    );
    const tail = await test$.reading.tail;
    assert.equal(test$.calls[0].tail, true, '第一次必须取尾窗，首屏只等这一次往返');
    assert.equal(tail.frames.length, 2, '尾窗立刻出屏');
    assert.equal(tail.settled, true, '终态帧在尾窗里，首屏就知道这一轮结束了');
    assert.equal(tail.cursor, cursor(91), '订阅游标取该轮真实尾帧');
    assert.equal(test$.painted().frames.length, 2, '回调与句柄拿到同一份尾窗');

    const read = await test$.reading.full;
    assert.equal(test$.calls[1].tail, false, '补齐前段仍然走整轮页，用序号对齐');
    assert.deepEqual(
        read.frames.map((item) => item.cursor),
        [cursor(1), cursor(50), cursor(51), cursor(90), cursor(91)],
        '补齐后按 durable 顺序整轮交出',
    );
    assert.equal(read.truncated, false, '前段补齐到尾窗起点即不算截断');
});

test('尾窗不足一页说明整轮都在里面，不再补读', async () => {
    const test$ = beginTailFirst(
        [page([frame('wire_event', cursor(1)), frame('turn_completed', cursor(2))], 2, false, cursor(2))],
        { limit: 800 },
    );
    const tail = await test$.reading.tail;
    const read = await test$.reading.full;

    assert.equal(test$.calls.length, 1, '一次往返就拿到整轮');
    assert.equal(read.frames.length, 2);
    assert.equal(tail.frames.length, 2, '首屏拿到的就是完整的一轮');
});

/*
 * 非最新那几条在跑的新轮只读尾窗：它们停在折叠形态，整轮回放等于为一段不渲染的过程付
 * 几十次串行往返。`tailOnly` 是这条读法的开关，`full` 因此就是尾窗本身。
 */
test('tailOnly：多条在跑的新轮只发 tail 请求，不补前段', async () => {
    const test$ = beginTailFirst(
        [
            // 尾窗满一页：若继续整轮补齐，下一张页一定会被取走。
            page([frame('wire_event', cursor(90)), frame('wire_event', cursor(91))], 91, true, cursor(91)),
            page([frame('wire_event', cursor(1)), frame('wire_event', cursor(2))], 2, false, cursor(2)),
        ],
        { limit: 2, tailOnly: true },
    );
    const tail = await test$.reading.tail;
    const full = await test$.reading.full;

    assert.equal(test$.calls.length, 1, '只允许一次尾窗往返');
    assert.equal(test$.calls[0].tail, true, '那一次请求必须是尾窗');
    assert.deepEqual(
        full.frames.map((item) => item.cursor),
        tail.frames.map((item) => item.cursor),
        'full 就是尾窗，不再补前段',
    );
    assert.equal(full.settled, false, '仍在跑的一轮不被冒充成收口');
});

test('合成终态帧不计入尾窗容量：恰好 limit-1 条真实帧不会被误判为还有前段', async () => {
    const test$ = beginTailFirst(
        [
            page(
                [
                    frame('wire_event', cursor(90)),
                    { eventType: 'interaction.status', payloadJson: '"understood"', cursor: null, replay: true },
                ],
                91,
                false,
                cursor(91),
            ),
        ],
        { limit: 2 },
    );
    const read = await test$.reading.full;
    assert.equal(test$.calls.length, 1, '真实帧只有一条，属于「整轮都在尾窗」');
    assert.equal(read.frames.length, 2);
});

test('前段超过自适应页数上限时如实标记截断，开头的帧仍然保留', async () => {
    const calls = [];
    let pages = 0;
    const fetchPage = async (interactionId, after, limit, tail) => {
        calls.push({ interactionId, after, limit, tail });
        if (tail) {
            return page([frame('wire_event', cursor(900)), frame('wire_event', cursor(901))], 901, false, cursor(901));
        }
        pages += 1;
        const start = after + 1;
        return page(
            [frame('wire_event', cursor(start)), frame('wire_event', cursor(start + 1))],
            start + 1,
            true,
            cursor(start + 1),
        );
    };
    const reading = readInteractionFramesTailFirst('i1', 1, {
        generation: () => 1,
        fetchPage,
        limit: 2,
        onTail: () => {},
    });
    await reading.tail;
    const read = await reading.full;

    assert.equal(pages, REPLAY_MAX_PAGES, '自适应预算同样受硬上限约束');
    assert.equal(read.truncated, true, '前段没补到尾窗起点必须如实标记');
    assert.equal(read.frames[0].cursor, cursor(1), '开头的帧不能因为截断被丢掉');
    assert.equal(read.frames[read.frames.length - 1].cursor, cursor(901), '结论必须留在末尾');
});

test('归档回收跳过的帧让这一轮如实标记「过程不完整」', async () => {
    const archived = page(
        [frame('wire_event', cursor(90)), frame('turn_completed', cursor(91))],
        91,
        false,
        cursor(91),
    );
    archived.skippedArchivedFrames = 3;
    const test$ = beginTailFirst([archived], { limit: 800 });
    const tail = await test$.reading.tail;
    const read = await test$.reading.full;
    assert.equal(tail.truncated, true, '尾窗一出屏就该说明部分正文已回收');
    assert.equal(read.truncated, true);
});

test('尾窗出屏之后目标切走：整次读取作废，不把补齐的正文交给新目标', async () => {
    const test$ = beginTailFirst(
        [
            page([frame('wire_event', cursor(90)), frame('wire_event', cursor(91))], 91, false, cursor(91)),
            page([frame('wire_event', cursor(1)), frame('wire_event', cursor(90))], 90, false, cursor(90)),
        ],
        {
            limit: 2,
            afterFetch: (current, calls) => (calls.length === 2 ? current + 1 : current),
        },
    );
    const tail = await test$.reading.tail;
    assert.notEqual(tail, null, '尾窗照常交出：要不要上屏由调用方按代次决定');
    assert.equal(await test$.reading.full, null, '切走之后补齐的结果一律作废');
});

test('页数预算按尾窗之前的帧数与页大小估算，并夹进硬上限', () => {
    assert.equal(replayPageBudget(0, 1000), 1, '没有前段时至少一次读取');
    assert.equal(replayPageBudget(500, 1000), 1, '不足一页按一页');
    assert.equal(replayPageBudget(2500, 1000), 3);
    assert.equal(replayPageBudget(100_000, 1000), REPLAY_MAX_PAGES, '病态长轮次不放大往返次数');
    assert.equal(replayPageBudget(2500, 0), 1, '页大小为非法值时不放大预算');
});

test('游标解析：老记录缺游标或不是 JSON 时给空，不猜序号', () => {
    assert.equal(cursorSequence(null), null);
    assert.equal(cursorSequence('c1'), null, '旧口径的纯文本游标不参与序号判定');
    assert.equal(cursorSequence('[1,2]'), null);
    assert.equal(cursorSequence(JSON.stringify({ generation: 'g', sequence: 0 })), 0);
    assert.equal(cursorSequence(JSON.stringify({ generation: 'g', sequence: 42 })), 42);
});
