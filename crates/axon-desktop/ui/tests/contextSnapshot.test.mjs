import test from 'node:test';
import assert from 'node:assert/strict';

import {
    contextPairText,
    contextSnapshotReading,
    formatUpdatedAgo,
    rolloverLineSummary,
    rolloverTokensText,
} from '../src/conversation/contextSnapshot.ts';
import { headerClusters } from '../src/conversation/metrics.ts';

/*
 * 头部上下文读数只认会话快照，分母受控回落到当前生效模型的配置。
 *
 * 旧口径读的是「本视图最近一轮带用量观测的用量」，并在新帧不带占用时沿用上一份：
 * 换窗之后头部因此一直停在换窗前的数字上。现在头部占用、窗口分母与换窗读数都取同一份
 * 带时间戳的快照；用量帧不带窗口时，分母回落到当前生效模型在配置目录里的窗口
 * （这一轮真正按它算的预算），两者都没有才是未知。
 */

const NOW = 1_760_000_000_000;

/** 换窗前后的一次快照：换窗前 825338，换窗后 31733，用量帧不带窗口。 */
function rolloverSnapshot(overrides = {}) {
    return {
        currentContextTokens: 31_733,
        contextWindow: null,
        composition: { conversationTokens: 30_000 },
        rolloverBeforeTokens: 825_338,
        rolloverAfterTokens: 31_733,
        updatedAtMs: NOW - 4_000,
        source: 'usage_frame+rollover_frame',
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

/** 指标条里的上下文段。 */
function contextSegment(input) {
    return headerClusters(input)
        .flatMap((cluster) => cluster.segments)
        .find((segment) => segment.kind === 'context');
}

test('header_context_follows_rollover_snapshot', () => {
    const snapshot = rolloverSnapshot();
    const reading = contextSnapshotReading(snapshot, NOW);
    assert.notEqual(reading, null, '有占用的快照必须读得出读数');

    const segment = contextSegment(baseInput({ contextSnapshot: reading }));
    assert.equal(segment.text, '31.7K/未知', '占用取快照，分母未知就写未知');
    assert.equal(segment.meta, '更新于 4 秒前', '次级读数是这份快照的更新时刻');
    assert.equal(
        reading.usedTokens,
        snapshot.rolloverAfterTokens,
        '头部占用跟到换窗后的实测读数，不再停在换窗前',
    );

    // 换窗进度行与头部共用同一份快照的 before/after：两边取的是同一个函数。
    const rolloverLine = rolloverTokensText(snapshot);
    assert.equal(rolloverLine, '换窗前 825K · 换窗后 31.7K');
    assert.equal(reading.rolloverText, rolloverLine, '两处读数的换窗文本同值');
    assert.equal(
        segment.title.includes(rolloverLine),
        true,
        '头部提示里带着换窗行那两半数字，同屏不会出现两套读数',
    );
    // 换窗进度行本身：运行侧那句「当前上下文 825338 tokens」被快照读数取代。
    const runtimeSummary = '正在开启新的记忆窗口 · 当前上下文 825338 tokens';
    assert.equal(
        rolloverLineSummary(runtimeSummary, reading.rolloverText),
        rolloverLine,
        '换窗行显示的就是头部提示里的同一份 before/after',
    );
    // 快照还没有换窗读数时保留运行侧的原始记录，不改写成别的数字。
    assert.equal(rolloverLineSummary(runtimeSummary, null), runtimeSummary);
});

test('header_context_unknown_when_snapshot_missing', () => {
    // 快照缺失（这条会话还没有任何可用读数）：整段不占位，不显示旧值也不显示 0。
    const missing = contextSnapshotReading(null, NOW);
    assert.equal(missing, null);
    assert.equal(contextSegment(baseInput({ contextSnapshot: missing })), undefined);

    // 快照存在但占用与窗口都未知、也没有换窗读数：同样整段不占位。
    const empty = contextSnapshotReading(
        rolloverSnapshot({
            currentContextTokens: null,
            rolloverBeforeTokens: null,
            rolloverAfterTokens: null,
            composition: null,
            updatedAtMs: null,
        }),
        NOW,
    );
    assert.equal(empty, null, '一个数字都没有的快照不承载上下文段');

    // 只有窗口、没有占用：不拿窗口冒充占用，也不写 0。
    const windowOnly = contextSnapshotReading(
        rolloverSnapshot({
            currentContextTokens: null,
            rolloverBeforeTokens: null,
            rolloverAfterTokens: null,
            contextWindow: 1_000_000,
        }),
        NOW,
    );
    assert.notEqual(windowOnly, null);
    assert.equal(windowOnly.usedTokens, null);
    assert.equal(
        contextSegment(baseInput({ contextSnapshot: windowOnly })),
        undefined,
        '没有占用读数时整段不占位，既不画 0 也不画 未知/1M',
    );

    // 更新时刻未知时说未知，不说「刚刚」。
    assert.equal(formatUpdatedAgo(null, NOW), '未知更新时刻');
    assert.equal(contextPairText(31_733, null), '31.7K/未知');
});

test('header_context_window_falls_back_to_configured_model', () => {
    // 用量帧通常不带窗口：分母回落当前生效模型在配置目录里的窗口。
    const configured = 1_000_000;
    const reading = contextSnapshotReading(
        rolloverSnapshot({ currentContextTokens: 46_200 }),
        NOW,
        configured,
    );
    assert.equal(reading.windowTokens, configured, '分母取当前生效模型的配置窗口');
    assert.equal(reading.usedTokens, 46_200, '占用仍只认快照');
    assert.equal(
        contextSegment(baseInput({ contextSnapshot: reading })).text,
        contextPairText(46_200, configured),
        '头部显示已用与配置窗口',
    );

    // 快照自带窗口时以实测为准，不被配置覆盖。
    const measured = contextSnapshotReading(
        rolloverSnapshot({ currentContextTokens: 46_200, contextWindow: 200_000 }),
        NOW,
        configured,
    );
    assert.equal(measured.windowTokens, 200_000, '运行时实测窗口优先于配置');

    // 快照与配置都没有窗口，才是未知。
    const unknown = contextSnapshotReading(
        rolloverSnapshot({ currentContextTokens: 46_200 }),
        NOW,
        null,
    );
    assert.equal(unknown.windowTokens, null);
    assert.equal(
        contextSegment(baseInput({ contextSnapshot: unknown })).text,
        '46.2K/未知',
        '两者都缺失时显示未知，不显示 0',
    );

    // 非正数的配置值不算分母：0 与缺失同义。
    const zero = contextSnapshotReading(
        rolloverSnapshot({ currentContextTokens: 46_200 }),
        NOW,
        0,
    );
    assert.equal(zero.windowTokens, null, '0 不是有效窗口');
});
