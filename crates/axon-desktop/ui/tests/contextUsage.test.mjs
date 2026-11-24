import test from 'node:test';
import assert from 'node:assert/strict';
import { contextUsageModel, segmentValueText } from '../src/conversation/contextUsage.ts';

/*
 * 上下文弹窗读的是会话快照：占用、窗口分母与构成都来自同一份带时间戳的读数。
 * 没有快照（或快照里没有占用）时连入口都不该出现；窗口未知时总量写未知而不是 0。
 */

/** 一份上下文快照读数；默认窗口 1M、已用 122K、剩余 878K。 */
function reading(overrides = {}) {
    return {
        usedTokens: 122_000,
        windowTokens: 1_000_000,
        remainingTokens: 878_000,
        composition: {
            systemPromptTokens: 1_800,
            toolDefinitionTokens: 19_100,
            conversationTokens: 6_200,
        },
        updatedText: '更新于 3 秒前',
        title: '上下文 122K/1M · 更新于 3 秒前',
        ...overrides,
    };
}

test('上下文弹窗给出已用百分比、实测总量与三类构成', () => {
    const model = contextUsageModel(reading());
    assert.equal(model.percentText, '12%');
    assert.equal(model.totalText, '122K / 1M');
    assert.equal(model.updatedText, '更新于 3 秒前', '读数更新时刻与指标条同一份事实');
    assert.deepEqual(
        model.segments.map(({ label }) => label),
        ['系统提示词', '工具定义', '对话消息', '其他'],
    );
    assert.deepEqual(
        model.segments.map(({ estimated }) => estimated),
        [true, true, true, false],
        '只有口径差额不是估算值',
    );
    assert.deepEqual(
        model.segments.map(({ color }) => typeof color === 'string' && color.length > 0),
        [true, true, true, true],
        '每段都带色块颜色，色带与明细共用',
    );
});

test('差额段吸收实测总量与估算总量的缺口，色带不会与标题对不上', () => {
    const model = contextUsageModel(reading());
    const total = model.segments.reduce((sum, segment) => sum + segment.tokens, 0);
    assert.equal(total, 122_000, '各段之和等于实测总量');
    assert.equal(model.segments.at(-1).tokens, 94_900);
});

test('估算总量已经覆盖实测总量时不造差额段', () => {
    const model = contextUsageModel(reading({ usedTokens: 20_000 }));
    assert.deepEqual(
        model.segments.map(({ kind }) => kind),
        ['systemPrompt', 'toolDefinitions', 'conversation'],
    );
});

test('微不足道的口径差额不单独占一行', () => {
    const model = contextUsageModel(
        reading({
            usedTokens: 27_113,
            windowTokens: 500_000,
            composition: {
                systemPromptTokens: 1_800,
                toolDefinitionTokens: 19_100,
                conversationTokens: 6_200,
            },
        }),
    );
    assert.deepEqual(
        model.segments.map(({ kind }) => kind),
        ['systemPrompt', 'toolDefinitions', 'conversation'],
        '13 个 token 的差额不配单独成行',
    );
    assert.equal(
        model.segments.reduce((sum, segment) => sum + segment.barPercent, 0),
        Number.parseInt(model.percentText, 10),
        '余量补给最大的一段，色带仍等于标题里的已用比例',
    );
});

test('没有构成观测时总量与百分比照常，明细留空由卡片显式降级', () => {
    const model = contextUsageModel(reading({ composition: null }));
    assert.deepEqual(model.segments, []);
    assert.equal(model.totalText, '122K / 1M', '总量仍然照实显示');
    assert.equal(model.percentText, '12%');
    assert.equal(model.usedRatio, 0.122);
});

test('窗口未知时不给百分比，总量分母写未知，色带按实测总量取标尺', () => {
    const model = contextUsageModel(reading({ windowTokens: null }));
    assert.equal(model.percentText, null);
    assert.equal(
        model.totalText,
        '122K / 未知',
        '分母未知就写未知：不写 0，也不拿模型目录的窗口值冒充',
    );
    assert.equal(model.usedRatio, 1);
    assert.equal(
        model.segments.reduce((sum, segment) => sum + segment.barPercent, 0),
        100,
        '标尺取实测总量，色带刚好铺满',
    );
});

test('色带宽度累加不超过 100', () => {
    const model = contextUsageModel(reading({ usedTokens: 2_000_000 }));
    assert.equal(model.percentText, '100%', '溢出时百分比夹到 100');
    assert.ok(model.segments.reduce((sum, segment) => sum + segment.barPercent, 0) <= 100);
});

test('估算值带波浪号，实测差额不带', () => {
    const model = contextUsageModel(reading());
    assert.equal(segmentValueText(model.segments[1]), '~19.1K');
    assert.equal(segmentValueText(model.segments[3]), '94.9K');
});

test('没有任何上下文观测时不出现弹窗模型', () => {
    assert.equal(contextUsageModel(null), null);
    assert.equal(contextUsageModel(reading({ usedTokens: null })), null);
    assert.equal(contextUsageModel(reading({ usedTokens: 0 })), null);
});
