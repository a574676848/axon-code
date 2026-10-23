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

test('LSP 与计划固定在指标条末尾', () => {
    const clusters = headerClusters({
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
    });

    assert.deepEqual(
        clusters.map((cluster) => cluster.kind),
        ['identity', 'budget', 'performance', 'aux'],
    );
    assert.deepEqual(
        clusters.at(-1).segments.map((segment) => segment.kind),
        ['lsp', 'plan'],
    );
});
