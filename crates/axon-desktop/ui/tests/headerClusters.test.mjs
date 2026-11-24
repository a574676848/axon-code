import test from 'node:test';
import assert from 'node:assert/strict';
import { headerClusters } from '../src/conversation/metrics.ts';

/*
 * 指标条的簇划分与出现条件。
 *
 * 上下文段只读会话上下文快照：占用与窗口分母同源，未知就写未知。首屏与没有快照时
 * 整段不占位，绝不画 0。
 */

/** 一份快照读数；默认已用 1K、窗口 10K、剩余 9K。 */
function snapshotReading(overrides = {}) {
    return {
        usedTokens: 1_000,
        windowTokens: 10_000,
        remainingTokens: 9_000,
        composition: { conversationTokens: 1_000 },
        updatedText: '更新于 2 秒前',
        title: '上下文 1K/10K · 剩余 9K · 更新于 2 秒前',
        ...overrides,
    };
}

/** 会话累计读数：默认有 10K 用量、3 分钟耗时。 */
function cumulative(overrides = {}) {
    return {
        totalTokens: 10_000,
        inputTokens: 2_000,
        cacheReadTokens: 7_500,
        cacheWriteTokens: 0,
        outputTokens: 500,
        reasoningTokens: 0,
        elapsedMs: 180_000,
        ...overrides,
    };
}

function baseInput(overrides = {}) {
    return {
        model: 'test-model',
        reasoningEffort: null,
        inSession: true,
        contextSnapshot: snapshotReading(),
        sessionCumulative: cumulative(),
        firstTokenAvgMs: 100,
        generationTpsX10: 100,
        plan: { completed: 1, total: 3 },
        lspActiveCount: 2,
        ...overrides,
    };
}

function cluster(input, kind) {
    return headerClusters(input).find((item) => item.kind === kind);
}

function auxKinds(input) {
    return cluster(input, 'aux').segments.map((segment) => segment.kind);
}

test('静息态没有身份簇，LSP 与计划固定在指标条末尾', () => {
    const clusters = headerClusters(baseInput());

    assert.deepEqual(
        clusters.map((item) => item.kind),
        ['session', 'performance', 'aux'],
        '指标条不写工作区路径，静息时身份簇整簇不占位',
    );
    assert.deepEqual(auxKinds(baseInput()), ['lsp', 'plan']);
});

test('MCP 台数与技能条数不再进指标条：它们已搬到侧栏连接行', () => {
    const input = baseInput();

    assert.deepEqual(
        auxKinds(input),
        ['lsp', 'plan'],
        '辅助簇只剩 LSP 与计划，档案、MCP 与技能读数归侧栏底栏',
    );
});

test('会话读数簇的顺序是 耗时 → 累计 → 命中率 → 上下文', () => {
    const session = cluster(baseInput(), 'session');

    assert.deepEqual(
        session.segments.map((segment) => segment.kind),
        ['elapsed', 'sessionTokens', 'cacheHit', 'context', 'remaining'],
    );
    assert.equal(session.segments[0].text, '耗时 3m 00s');
    assert.equal(session.segments[1].text, '累计 10K');
    assert.equal(session.segments[1].title, '本会话累计消耗 10K token');
    // 命中率按整条会话累计：7500 / (2000 + 7500 + 0)
    assert.equal(session.segments[2].text, '命中率 78.94%');
    assert.equal(session.segments[3].text, '1K/10K', '上下文段取快照的已用与窗口');
    assert.equal(session.segments[4].text, '剩余 9K', '剩余段取快照派生值');
    assert.equal(
        session.segments[3].meta,
        undefined,
        '指标条只给已用/窗口，行尾不再挂「更新于」这类新鲜度后缀',
    );
});

test('零耗时的时候不占位，其余三段照常', () => {
    const session = cluster(baseInput({ sessionCumulative: cumulative({ elapsedMs: 0 }) }), 'session');

    assert.deepEqual(
        session.segments.map((segment) => segment.kind),
        ['sessionTokens', 'cacheHit', 'context', 'remaining'],
    );
});

test('首屏没有会变的读数时，指标条整行不占位', () => {
    const firstScreen = baseInput({
        inSession: false,
        sessionCumulative: null,
        contextSnapshot: null,
        firstTokenAvgMs: null,
        generationTpsX10: null,
        plan: null,
        lspActiveCount: 0,
    });

    assert.deepEqual(
        headerClusters(firstScreen),
        [],
        '首屏只剩辅助簇，而 MCP 与技能已搬到侧栏，因此没有可画的簇',
    );
});

test('没有用量观测时累计与命中率都不占位，上下文另有来源', () => {
    const fresh = cluster(
        baseInput({
            contextSnapshot: null,
            sessionCumulative: cumulative({
                totalTokens: 0,
                inputTokens: 0,
                cacheReadTokens: 0,
                outputTokens: 0,
                elapsedMs: 0,
            }),
        }),
        'session',
    );
    assert.equal(fresh, undefined, '一个 token 都没观测到、也没有快照时整簇不占位');

    const running = cluster(baseInput({ contextSnapshot: null, sessionCumulative: null }), 'session');
    assert.equal(running, undefined, '基线未取回且没有观测时整簇也不占位');
});

test('只有耗时没有用量时只留耗时一段', () => {
    const session = cluster(
        baseInput({
            contextSnapshot: null,
            sessionCumulative: cumulative({
                totalTokens: 0,
                inputTokens: 0,
                cacheReadTokens: 0,
                outputTokens: 0,
                elapsedMs: 90_000,
            }),
        }),
        'session',
    );

    assert.deepEqual(
        session.segments.map((segment) => segment.kind),
        ['elapsed'],
        '在跑的第一轮还没有用量帧时，只呈现耗时',
    );
    assert.equal(session.segments[0].text, '耗时 1m 30s');
});

test('窗口未知时上下文的分母写未知，不写 0', () => {
    const session = cluster(
        baseInput({ contextSnapshot: snapshotReading({ windowTokens: null }) }),
        'session',
    );
    const context = session.segments.find((segment) => segment.kind === 'context');

    assert.equal(context.text, '1K/未知');
    assert.equal(context.tone, 'default', '没有窗口就谈不上快满了，不给告警色调');
});

test('快照为 null 时上下文整段不占位，不显示旧值也不显示 0', () => {
    const session = cluster(baseInput({ contextSnapshot: null }), 'session');
    const context = session.segments.find((segment) => segment.kind === 'context');

    assert.equal(context, undefined);
    assert.equal(
        session.segments.some((segment) => segment.text.includes('0/')),
        false,
        '不得出现任何以 0 开头的占用读数',
    );
});
