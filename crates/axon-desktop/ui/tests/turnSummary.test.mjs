import test from 'node:test';
import assert from 'node:assert/strict';
import { turnSummaryRows, turnSummaryText } from '../src/conversation/turnSummary.ts';

const usage = {
    inputTokens: 1000,
    cacheReadInputTokens: 810,
    cacheWriteInputTokens: 0,
    outputTokens: 2000,
    reasoningOutputTokens: 500,
    currentContextTokens: 3000,
};

test('轮次分割线与 TUI 同项同序：用时、Token、入、缓存命中、出、推理、命中率、首字、速率', () => {
    const cell = {
        id: 'summary',
        role: 'summary',
        durationMs: 103_000,
        usage,
        firstTokenAvgMs: 8_460,
        generationTpsX10: 123,
    };
    assert.deepEqual(
        turnSummaryRows(cell).map(({ label }) => label),
        ['用时', 'Token', '入', '缓存命中', '出', '推理', '命中率', '首字', '速率'],
    );
    assert.equal(
        turnSummaryText(cell),
        '用时 1m 43s · Token 4.31K · 入 1K · 缓存命中 810 · 出 2K · 推理 500 · 命中率 44.75% · 首字 8.46s · 速率 12.3 tok/s',
    );
});

test('没有观测到的指标不出现，也不占位', () => {
    const bare = {
        id: 'summary',
        role: 'summary',
        durationMs: 2_500,
        usage: null,
        firstTokenAvgMs: null,
        generationTpsX10: null,
    };
    assert.deepEqual(turnSummaryRows(bare).map(({ label }) => label), ['用时']);
    assert.equal(turnSummaryText(bare), '用时 2.5s');

    // 一轮什么都没测到时，分割线整行不渲染。
    const empty = { id: 'summary', role: 'summary', durationMs: null, usage: null, firstTokenAvgMs: null, generationTpsX10: null };
    assert.equal(turnSummaryText(empty), '');
});
