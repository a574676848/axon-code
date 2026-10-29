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

function baseInput(overrides = {}) {
    return {
        working: false,
        animationTick: 0,
        model: 'test-model',
        cwd: '/workspace',
        reasoningEffort: null,
        usage,
        contextWindow: 10_000,
        firstTokenAvgMs: 100,
        generationTpsX10: 100,
        elapsedMs: 1_000,
        plan: { completed: 1, total: 3 },
        lspActiveCount: 2,
        ...overrides,
    };
}

function auxKinds(input) {
    return headerClusters(input)
        .at(-1)
        .segments.map((segment) => segment.kind);
}

test('LSP 与计划固定在指标条末尾', () => {
    const clusters = headerClusters(baseInput());

    assert.deepEqual(
        clusters.map((cluster) => cluster.kind),
        ['identity', 'budget', 'performance', 'aux'],
    );
    assert.deepEqual(auxKinds(baseInput()), ['lsp', 'plan']);
});

test('技能条数紧跟 MCP，同为辅助读数', () => {
    const input = baseInput({
        mcp: { totalCount: 2, loadedCount: 2, failedCount: 0, error: null },
        skillCount: 7,
    });

    assert.deepEqual(auxKinds(input), ['lsp', 'mcp', 'skill', 'plan']);

    const skill = headerClusters(input).at(-1).segments[2];
    assert.equal(skill.text, '技能 7');
    assert.equal(skill.title, '共 7 项已安装技能');
    assert.equal(skill.tone, 'default');
    assert.equal(skill.icon, 'skill');
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
