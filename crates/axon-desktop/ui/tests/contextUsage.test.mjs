import test from 'node:test';
import assert from 'node:assert/strict';
import { contextUsageModel } from '../src/conversation/contextUsage.ts';

const usage = (overrides = {}) => ({
    inputTokens: 0,
    cacheReadInputTokens: 0,
    cacheWriteInputTokens: 0,
    outputTokens: 0,
    reasoningOutputTokens: 0,
    currentContextTokens: 122_000,
    systemPromptTokens: 1_800,
    toolDefinitionTokens: 19_100,
    conversationTokens: 6_200,
    ...overrides,
});

test('上下文弹窗给出实测总量与三类构成', () => {
    const model = contextUsageModel(usage(), 1_000_000);
    assert.equal(model.totalText, '122K / 1M');
    assert.deepEqual(
        model.rows.map(({ label }) => label),
        ['系统提示词', '工具定义', '对话消息', '其他'],
    );
    assert.deepEqual(
        model.rows.map(({ value }) => value.startsWith('~')),
        [true, true, true, false],
        '只有口径差额不是估算值',
    );
});

test('差额行吸收实测总量与估算总量的缺口', () => {
    const tokensOf = (value) => Number(value.replace('~', '').replace('K', '')) * 1_000;
    const model = contextUsageModel(usage(), 1_000_000);
    const rows = model.rows.slice(0, 3).reduce((sum, row) => sum + tokensOf(row.value), 0);
    assert.equal(rows, 27_100, '前三行就是本地估算的三项');
    assert.equal(model.rows.at(-1).value, '94.9K', '差额补齐到实测总量');
});

test('估算总量已经覆盖实测总量时不造差额行', () => {
    const model = contextUsageModel(usage({ currentContextTokens: 20_000 }), 1_000_000);
    assert.deepEqual(
        model.rows.map(({ label }) => label),
        ['系统提示词', '工具定义', '对话消息'],
    );
});

test('微不足道的口径差额不单独占一行', () => {
    const model = contextUsageModel(
        usage({ currentContextTokens: 27_113, conversationTokens: 6_200 }),
        500_000,
    );
    assert.deepEqual(
        model.rows.map(({ label }) => label),
        ['系统提示词', '工具定义', '对话消息'],
        '13 个 token 的差额不配单独成行',
    );
});

test('没有构成观测时刻度仍在，明细留空由卡片显式降级', () => {
    const model = contextUsageModel(
        usage({ systemPromptTokens: 0, toolDefinitionTokens: 0, conversationTokens: 0 }),
        1_000_000,
    );
    assert.deepEqual(model.rows, []);
    assert.equal(model.totalText, '122K / 1M', '总量仍然照实显示');
    assert.equal(model.usedRatio, 0.122);
});

test('无窗口定义时只报已用，环满格', () => {
    const model = contextUsageModel(usage(), null);
    assert.equal(model.totalText, '122K');
    assert.equal(model.usedRatio, 1);
});

test('溢出窗口时比例夹到 1', () => {
    const model = contextUsageModel(usage({ currentContextTokens: 2_000_000 }), 1_000_000);
    assert.equal(model.usedRatio, 1);
});

test('没有任何上下文观测时不出现弹窗模型', () => {
    assert.equal(contextUsageModel(null, 1_000_000), null);
    assert.equal(contextUsageModel(usage({ currentContextTokens: 0 }), 1_000_000), null);
});
