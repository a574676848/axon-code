import test from 'node:test';
import assert from 'node:assert/strict';
import { applyUsage, foldStreamUsage } from '../src/conversation/transcript/cells.ts';
import { EMPTY_USAGE } from '../src/conversation/transcript/state.ts';

/** 一轮的最小事实：用量累计与在跑流快照。 */
const turn = () => ({
    usage: { ...EMPTY_USAGE },
    usageFolded: { ...EMPTY_USAGE },
    usageStream: null,
});

/** 一条用量帧：只写测试关心的字段。 */
const frame = (fields) => ({
    inputTokens: 0,
    outputTokens: 0,
    cachedInputTokens: 0,
    ...fields,
});

test('同一条流的重复用量帧按同一快照覆盖，不翻倍', () => {
    const first = frame({ inputTokens: 596, outputTokens: 69, reasoningOutputTokens: 1525 });
    let state = applyUsage(turn(), first);
    const once = { ...state.usage };
    state = applyUsage(state, first);

    assert.deepEqual(state.usage, once);
    assert.equal(state.usage.outputTokens, 69);
    assert.equal(state.usage.reasoningOutputTokens, 1525);
});

test('流收口后跨流累加，前一条流的快照不被替换掉', () => {
    let state = applyUsage(
        turn(),
        frame({ inputTokens: 596, outputTokens: 69, reasoningOutputTokens: 1525 }),
    );
    state = foldStreamUsage(state);
    state = applyUsage(
        state,
        frame({
            inputTokens: 3092,
            cachedInputTokens: 60984,
            outputTokens: 3,
            reasoningOutputTokens: 164,
            currentContextTokens: 64049,
        }),
    );

    assert.equal(state.usage.outputTokens, 72);
    assert.equal(state.usage.reasoningOutputTokens, 1689);
    assert.equal(state.usage.inputTokens, 596 + 3092);
    assert.equal(state.usage.cacheReadInputTokens, 60984);
    // 上下文占用是「最近一次请求」的快照，取在跑流的值。
    assert.equal(state.usage.currentContextTokens, 64049);
    assert.equal(state.usageFolded.outputTokens, 69);
});

test('流内增长中的快照按最新值覆盖，不累加', () => {
    let state = applyUsage(turn(), frame({ outputTokens: 10 }));
    state = applyUsage(state, frame({ outputTokens: 40 }));

    assert.equal(state.usage.outputTokens, 40);
});

test('没有在跑流时收口是空操作', () => {
    const before = turn();
    assert.equal(foldStreamUsage(before), before);
});
