import test from 'node:test';
import assert from 'node:assert/strict';

import {
    contextPairText,
    contextSnapshotReading,
    formatUpdatedAgo,
} from '../src/conversation/contextSnapshot.ts';
import { headerClusters } from '../src/conversation/metrics.ts';

/*
 * 头部上下文读数只认会话快照，分母受控回落到当前生效模型的配置。
 *
 * 快照里只有用量帧给出的实测占用与窗口：换窗不再产生任何前后读数，剩余量由
 * 「窗口减占用」派生。占用未知时上下文段与剩余段一起消失，绝不拿 0 或旧值顶替。
 */

const NOW = 1_760_000_000_000;

/** 一份用量帧形状的快照。 */
function usageSnapshot(overrides = {}) {
    return {
        currentContextTokens: 31_733,
        contextWindow: null,
        composition: { conversationTokens: 30_000 },
        updatedAtMs: NOW - 4_000,
        source: 'usage_frame',
        ...overrides,
    };
}

/** 指标条输入；`headerClusters` 只消费这几项。 */
function baseInput(overrides = {}) {
    return {
        model: 'test-model',
        reasoningEffort: null,
        inSession: true,
        contextSnapshot: null,
        sessionCumulative: {
            totalTokens: 10_000,
            inputTokens: 2_000,
            cacheReadTokens: 7_500,
            cacheWriteTokens: 0,
            outputTokens: 500,
            reasoningTokens: 0,
            elapsedMs: 180_000,
        },
        firstTokenAvgMs: null,
        generationTpsX10: null,
        plan: null,
        lspActiveCount: 0,
        ...overrides,
    };
}

/** 指标条里的某一段。 */
function segmentOf(input, kind) {
    return headerClusters(input)
        .flatMap((cluster) => cluster.segments)
        .find((segment) => segment.kind === kind);
}

test('header_shows_remaining_after_context', () => {
    const reading = contextSnapshotReading(usageSnapshot({ contextWindow: 200_000 }), NOW);
    assert.notEqual(reading, null, '有占用的快照必须读得出读数');
    assert.equal(reading.remainingTokens, 168_267, '剩余 = 窗口减实测占用');

    const input = baseInput({ contextSnapshot: reading });
    const context = segmentOf(input, 'context');
    const remaining = segmentOf(input, 'remaining');
    assert.equal(context.text, '31.7K/200K', '占用取快照，分母取窗口');
    assert.equal(remaining.text.startsWith('剩余 '), true, `剩余段必须带标签：${remaining.text}`);
    assert.equal(
        remaining.title.includes('窗口减去'),
        true,
        '剩余段提示必须说明口径',
    );

    // 剩余量在段序上紧跟在上下文之后。
    const kinds = headerClusters(input)
        .flatMap((cluster) => cluster.segments)
        .map((segment) => segment.kind);
    assert.equal(kinds.indexOf('remaining'), kinds.indexOf('context') + 1);
});

test('header_context_unknown_when_snapshot_missing', () => {
    // 快照缺失（这条会话还没有任何可用读数）：整段不占位，不显示旧值也不显示 0。
    const missing = contextSnapshotReading(null, NOW);
    assert.equal(missing, null);
    assert.equal(segmentOf(baseInput({ contextSnapshot: missing }), 'context'), undefined);
    assert.equal(segmentOf(baseInput({ contextSnapshot: missing }), 'remaining'), undefined);

    // 快照存在但占用与窗口都未知：同样整段不占位。
    const empty = contextSnapshotReading(
        usageSnapshot({ currentContextTokens: null, composition: null, updatedAtMs: null }),
        NOW,
    );
    assert.equal(empty, null, '一个数字都没有的快照不承载上下文段');

    // 只有窗口、没有占用：不拿窗口冒充占用，也不写 0，剩余段同样不出现。
    const windowOnly = contextSnapshotReading(
        usageSnapshot({ currentContextTokens: null, contextWindow: 1_000_000 }),
        NOW,
    );
    assert.notEqual(windowOnly, null);
    assert.equal(windowOnly.usedTokens, null);
    assert.equal(windowOnly.remainingTokens, null);
    assert.equal(segmentOf(baseInput({ contextSnapshot: windowOnly }), 'context'), undefined);
    assert.equal(segmentOf(baseInput({ contextSnapshot: windowOnly }), 'remaining'), undefined);

    // 更新时刻未知时说未知，不说「刚刚」。
    assert.equal(formatUpdatedAgo(null, NOW), '未知更新时刻');
    assert.equal(contextPairText(31_733, null), '31.7K/未知');
});

test('header_context_window_falls_back_to_configured_model', () => {
    // 用量帧通常不带窗口：分母回落当前生效模型在配置目录里的窗口。
    const configured = 1_000_000;
    const reading = contextSnapshotReading(
        usageSnapshot({ currentContextTokens: 46_200 }),
        NOW,
        configured,
    );
    assert.equal(reading.windowTokens, configured, '分母取当前生效模型的配置窗口');
    assert.equal(reading.usedTokens, 46_200, '占用仍只认快照');
    assert.equal(reading.remainingTokens, 953_800, '剩余随回退后的窗口一起派生');
    assert.equal(
        segmentOf(baseInput({ contextSnapshot: reading }), 'context').text,
        contextPairText(46_200, configured),
        '头部显示已用与配置窗口',
    );

    // 快照自带窗口时以实测为准，不被配置覆盖。
    const measured = contextSnapshotReading(
        usageSnapshot({ currentContextTokens: 46_200, contextWindow: 200_000 }),
        NOW,
        configured,
    );
    assert.equal(measured.windowTokens, 200_000, '运行时实测窗口优先于配置');
    assert.equal(measured.remainingTokens, 153_800);

    // 快照与配置都没有窗口，才是未知；剩余量也随之缺失。
    const unknown = contextSnapshotReading(usageSnapshot({ currentContextTokens: 46_200 }), NOW, null);
    assert.equal(unknown.windowTokens, null);
    assert.equal(unknown.remainingTokens, null);
    assert.equal(
        segmentOf(baseInput({ contextSnapshot: unknown }), 'context').text,
        '46.2K/未知',
        '两者都缺失时显示未知，不显示 0',
    );
    assert.equal(segmentOf(baseInput({ contextSnapshot: unknown }), 'remaining'), undefined);

    // 非正数的配置值不算分母：0 与缺失同义。
    const zero = contextSnapshotReading(usageSnapshot({ currentContextTokens: 46_200 }), NOW, 0);
    assert.equal(zero.windowTokens, null, '0 不是有效窗口');
});

test('remaining_never_goes_negative', () => {
    // 已用超过窗口时收敛到 0，不显示负数。
    const reading = contextSnapshotReading(
        usageSnapshot({ currentContextTokens: 220_000, contextWindow: 200_000 }),
        NOW,
    );
    assert.equal(reading.remainingTokens, 0);
});
