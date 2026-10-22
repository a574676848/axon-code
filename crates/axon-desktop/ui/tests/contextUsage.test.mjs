import test from 'node:test';
import assert from 'node:assert/strict';
import { contextUsageModel, segmentValueText } from '../src/conversation/contextUsage.ts';

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

test('上下文弹窗给出已用百分比、实测总量与三类构成', () => {
    const model = contextUsageModel(usage(), 1_000_000);
    assert.equal(model.usedPercent, 12);
    assert.equal(model.windowText, '122K / 1M');
    assert.deepEqual(
        model.segments.map(({ label }) => label),
        ['系统提示词', '工具定义', '对话消息', '其他'],
    );
    assert.deepEqual(
        model.segments.map(({ estimated }) => estimated),
        [true, true, true, false],
        '只有口径差额不是估算值',
    );
});

test('差额段吸收实测总量与估算总量的缺口，色带不会与标题对不上', () => {
    const model = contextUsageModel(usage(), 1_000_000);
    const total = model.segments.reduce((sum, segment) => sum + segment.tokens, 0);
    assert.equal(total, 122_000, '各段之和等于 Provider 实测总量');
    assert.equal(model.segments.at(-1).tokens, 94_900);
});

test('估算总量已经覆盖实测总量时不造差额段', () => {
    const model = contextUsageModel(
        usage({ currentContextTokens: 20_000 }),
        1_000_000,
    );
    assert.deepEqual(
        model.segments.map(({ kind }) => kind),
        ['systemPrompt', 'toolDefinitions', 'conversation'],
    );
});

test('微不足道的口径差额不单独占一行', () => {
    const model = contextUsageModel(
        usage({ currentContextTokens: 27_113, conversationTokens: 6_200 }),
        500_000,
    );
    assert.deepEqual(
        model.segments.map(({ kind }) => kind),
        ['systemPrompt', 'toolDefinitions', 'conversation'],
        '13 个 token 的差额不配单独成行',
    );
    assert.equal(
        model.segments.reduce((sum, segment) => sum + segment.barPercent, 0),
        model.usedPercent,
        '余量补给最大的一段，色带仍等于标题里的已用比例',
    );
});

test('没有构成观测时显式降级，不填占位数值', () => {
    const model = contextUsageModel(
        usage({ systemPromptTokens: 0, toolDefinitionTokens: 0, conversationTokens: 0 }),
        1_000_000,
    );
    assert.equal(model.hasComposition, false);
    assert.deepEqual(model.segments, []);
    assert.equal(model.usedPercent, 12, '总量仍然照实显示');
});

test('无窗口定义时不给百分比，色带按实测总量取标尺', () => {
    const model = contextUsageModel(usage(), null);
    assert.equal(model.usedPercent, null);
    assert.equal(model.windowText, '122K');
    assert.equal(
        model.segments.reduce((sum, segment) => sum + segment.barPercent, 0),
        100,
        '标尺取实测总量，色带刚好铺满',
    );
});

test('色带的彩色部分恒等于标题里的已用比例', () => {
    const model = contextUsageModel(usage(), 1_000_000);
    assert.equal(
        model.segments.reduce((sum, segment) => sum + segment.barPercent, 0),
        model.usedPercent,
    );
});

test('色带宽度累加不超过 100', () => {
    const model = contextUsageModel(usage({ currentContextTokens: 2_000_000 }), 1_000_000);
    assert.equal(model.usedPercent, 100, '溢出时百分比夹到 100');
    assert.ok(model.segments.reduce((sum, segment) => sum + segment.barPercent, 0) <= 100);
});

test('估算值带波浪号，实测差额不带', () => {
    const model = contextUsageModel(usage(), 1_000_000);
    assert.equal(segmentValueText(model.segments[1]), '~19.1K');
    assert.equal(segmentValueText(model.segments[3]), '94.9K');
});

test('没有任何上下文观测时不出现弹窗模型', () => {
    assert.equal(contextUsageModel(null, 1_000_000), null);
    assert.equal(contextUsageModel(usage({ currentContextTokens: 0 }), 1_000_000), null);
});
