import test from 'node:test';
import assert from 'node:assert/strict';
import { headerClusters } from '../src/conversation/metrics.ts';

const usage = {
    inputTokens: 1000,
    cacheReadInputTokens: 500,
    cacheWriteInputTokens: 0,
    outputTokens: 100,
    reasoningOutputTokens: 0,
    currentContextTokens: 1000,
    systemPromptTokens: 0,
    toolDefinitionTokens: 0,
    conversationTokens: 1000,
};

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
        working: false,
        animationTick: 0,
        model: 'test-model',
        reasoningEffort: null,
        inSession: true,
        usage,
        sessionCumulative: cumulative(),
        contextWindow: 10_000,
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

test('技能条数紧跟 MCP，同为辅助读数', () => {
    const input = baseInput({
        mcp: { totalCount: 2, loadedCount: 2, failedCount: 0, error: null },
        skillCount: 7,
    });

    assert.deepEqual(auxKinds(input), ['lsp', 'mcp', 'skill', 'plan']);

    const skill = cluster(input, 'aux').segments[2];
    assert.equal(skill.text, '技能 7');
    assert.equal(skill.title, '共 7 项已安装技能');
    assert.equal(skill.tone, 'default');
    assert.equal(skill.icon, 'skill');
});

test('会话读数簇的顺序是 耗时 → 累计 → 命中率 → 上下文', () => {
    const session = cluster(baseInput(), 'session');

    assert.deepEqual(
        session.segments.map((segment) => segment.kind),
        ['elapsed', 'sessionTokens', 'cacheHit', 'context'],
    );
    assert.equal(session.segments[0].text, '耗时 3m 00s');
    assert.equal(session.segments[1].text, '累计 10K');
    assert.equal(session.segments[1].title, '本会话累计消耗 10K token');
    // 命中率按整条会话累计：7500 / (2000 + 7500 + 0)
    assert.equal(session.segments[2].text, '命中率 78.94%');
    assert.equal(session.segments[3].text, '1K/10K', '上下文段仍是最近一次请求的占用');
});

test('零耗时的时候不占位，其余三段照常', () => {
    const session = cluster(baseInput({ sessionCumulative: cumulative({ elapsedMs: 0 }) }), 'session');

    assert.deepEqual(
        session.segments.map((segment) => segment.kind),
        ['sessionTokens', 'cacheHit', 'context'],
    );
});

test('首屏只留辅助读数：会话读数与上下文整组不占位', () => {
    const firstScreen = baseInput({
        inSession: false,
        sessionCumulative: null,
        usage: null,
        firstTokenAvgMs: null,
        generationTpsX10: null,
        plan: null,
        lspActiveCount: 0,
        mcp: { totalCount: 3, loadedCount: 3, failedCount: 0, error: null },
        skillCount: 32,
    });

    assert.deepEqual(
        headerClusters(firstScreen).map((item) => item.kind),
        ['aux'],
        '还没进入会话时指标条只剩 MCP 与技能',
    );
    assert.deepEqual(auxKinds(firstScreen), ['mcp', 'skill']);
});

test('没有用量观测时累计与命中率都不占位，与上下文同一生命周期', () => {
    const fresh = cluster(
        baseInput({
            usage: null,
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
    assert.equal(fresh, undefined, '一个 token 都没观测到时整簇不占位');

    const running = cluster(baseInput({ usage: null, sessionCumulative: null }), 'session');
    assert.equal(running, undefined, '基线未取回且没有观测时整簇也不占位');
});

test('只有耗时没有用量时只留耗时一段', () => {
    const session = cluster(
        baseInput({
            usage: null,
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

test('窗口未知时上下文只报已用', () => {
    const session = cluster(baseInput({ contextWindow: null }), 'session');
    const context = session.segments.find((segment) => segment.kind === 'context');

    assert.equal(context.text, '1K');
});

test('技能清单未就绪或零条时都不占位', () => {
    for (const skillCount of [null, undefined, 0]) {
        assert.deepEqual(
            auxKinds(baseInput({ skillCount })),
            ['lsp', 'plan'],
            `skillCount=${String(skillCount)} 时不应出现技能段`,
        );
    }
});
