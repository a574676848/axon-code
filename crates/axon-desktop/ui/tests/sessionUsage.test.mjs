import test from 'node:test';
import assert from 'node:assert/strict';

import {
    hasUsageObservation,
    lastObservedUsage,
    EMPTY_SESSION_USAGE,
    cumulativeCacheHitBasisPoints,
    sessionCumulative,
} from '../src/conversation/sessionUsage.ts';

const EMPTY = {
    inputTokens: 0,
    cacheReadInputTokens: 0,
    cacheWriteInputTokens: 0,
    outputTokens: 0,
    reasoningOutputTokens: 0,
    currentContextTokens: 0,
    systemPromptTokens: 0,
    toolDefinitionTokens: 0,
    conversationTokens: 0,
};

function usage(overrides = {}) {
    return { ...EMPTY, ...overrides };
}

test('新一轮还没有用量时沿用上一轮，指标条不清零', () => {
    const observed = usage({ inputTokens: 1_000, cacheReadInputTokens: 500, currentContextTokens: 336_000 });

    assert.equal(lastObservedUsage([{ usage: observed }, { usage: EMPTY }]), observed);
});

test('整条会话一次都没观测过时返回 null，交给零读数渲染', () => {
    assert.equal(lastObservedUsage([]), null);
    assert.equal(lastObservedUsage([{ usage: EMPTY }, { usage: EMPTY }]), null);
});

test('最新一轮已有观测就直接用它', () => {
    const older = usage({ inputTokens: 900 });
    const newest = usage({ inputTokens: 1_200 });

    assert.equal(lastObservedUsage([{ usage: older }, { usage: newest }]), newest);
});

test('只有上下文总量也算观测：Provider 可以先报窗口占用再报分桶', () => {
    const contextOnly = usage({ currentContextTokens: 1_024 });

    assert.equal(hasUsageObservation(contextOnly), true);
    assert.equal(hasUsageObservation(EMPTY), false);
    assert.equal(lastObservedUsage([{ usage: EMPTY }, { usage: contextOnly }]), contextOnly);
});

/** 一轮最小投影：累计读数只读标识、用量与耗时。 */
function turn(id, buckets, elapsedMs = 0) {
    return { id, usage: usage(buckets), elapsedMs };
}

/** 会话累计基线：默认 6M 用量、1 小时耗时。 */
function baseline(overrides = {}) {
    return {
        usage: {
            totalTokens: 6_000_000,
            inputTokens: 1_000_000,
            cacheReadTokens: 4_800_000,
            cacheWriteTokens: 0,
            outputTokens: 200_000,
            reasoningTokens: 0,
        },
        elapsedMs: 3_600_000,
        turnIds: new Set(['history-1']),
        ...overrides,
    };
}

test('基线未取回时累计为空，指标条不画这一段', () => {
    assert.equal(sessionCumulative([turn('t1', { outputTokens: 100 })], null), null);
});

test('基线覆盖的历史轮次不重复计入', () => {
    const totals = sessionCumulative(
        [turn('history-1', { outputTokens: 5_000_000 }, 999)],
        baseline(),
    );

    assert.equal(totals.totalTokens, 6_000_000);
    assert.equal(totals.elapsedMs, 3_600_000);
});

test('基线之后发生的轮次按增量叠加，各桶与耗时同口径', () => {
    const totals = sessionCumulative(
        [
            turn('history-1', { outputTokens: 900 }, 5_000),
            turn('pending-1', { inputTokens: 200, cacheReadInputTokens: 50, outputTokens: 50 }, 750),
        ],
        baseline(),
    );

    assert.equal(totals.totalTokens, 6_000_300);
    assert.equal(totals.inputTokens, 1_000_200);
    assert.equal(totals.cacheReadTokens, 4_800_050);
    assert.equal(totals.outputTokens, 200_050);
    assert.equal(totals.elapsedMs, 3_600_750);
});

test('打开时仍在生成的轮次整条由本地增量承担，收口后不回落', () => {
    const running = [turn('history-1', { outputTokens: 400 }, 900)];

    const first = sessionCumulative(running, baseline(), new Set(['history-1']));
    assert.equal(first.totalTokens, 6_000_400);
    assert.equal(first.elapsedMs, 3_600_900);
    assert.deepEqual(
        sessionCumulative(running, baseline(), new Set(['history-1'])),
        first,
        '收口只是换了轮次状态，读数口径不变',
    );
});

test('命中率按整条会话累计，不是最近一轮', () => {
    const totals = sessionCumulative([], baseline());
    // 4,800,000 / (1,000,000 + 4,800,000 + 0)
    assert.equal(cumulativeCacheHitBasisPoints(totals), 8_275);
});

test('新建会话没有历史时各项都读作 0', () => {
    const empty = {
        usage: EMPTY_SESSION_USAGE,
        elapsedMs: 0,
        turnIds: new Set(),
    };

    assert.deepEqual(sessionCumulative([], empty), { ...EMPTY_SESSION_USAGE, elapsedMs: 0 });
});