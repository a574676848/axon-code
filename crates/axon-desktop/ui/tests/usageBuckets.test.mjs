import test from 'node:test';
import assert from 'node:assert/strict';
import {
    bucketStart,
    calendarWeeks,
    classTotals,
    granularityForPeriod,
    healthRows,
    ladderRows,
    niceTicks,
    scatterGeometry,
    stackColumns,
    trendBuckets,
} from '../src/components/usage/usageBuckets.ts';

/** 一条时间序列点：只填测试关心的字段，其余按 0。 */
const point = (timestampMs, overrides = {}) => ({
    timestampMs,
    uncachedInputTokens: 0,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    outputTokens: 0,
    reasoningTokens: 0,
    requestCount: 0,
    costMicros: 0,
    ...overrides,
});

const HOUR = 3_600_000;
const DAY = 86_400_000;

test('时间桶按本地日历对齐：小时、天、周各有明确边界', () => {
    const noon = new Date(2026, 9, 21, 12, 34, 56).getTime();
    const hour = new Date(bucketStart(noon, 'hour'));
    assert.equal(hour.getMinutes(), 0);
    assert.equal(hour.getSeconds(), 0);
    assert.equal(hour.getHours(), 12);

    const day = new Date(bucketStart(noon, 'day'));
    assert.equal(day.getHours(), 0);
    assert.equal(day.getDate(), 21);

    const sixHour = new Date(bucketStart(noon, 'sixHour'));
    assert.equal(sixHour.getHours() % 6, 0);

    const week = new Date(bucketStart(noon, 'week'));
    assert.equal(week.getDay(), 1, '一周从周一开始');
    assert.equal(week.getHours(), 0);
});

test('周期决定粒度，长窗口自动降档', () => {
    assert.equal(granularityForPeriod('today'), 'hour');
    assert.equal(granularityForPeriod('7d'), 'sixHour');
    assert.equal(granularityForPeriod('30d'), 'day');

    const now = new Date(2026, 9, 21, 16, 0, 0).getTime();
    const points = [point(now - 2 * HOUR, { uncachedInputTokens: 100 })];
    // 一年的小时桶远超上限，必须自己降到周桶而不是画一千根柱子。
    const long = trendBuckets(points, 'hour', now - 365 * DAY, now);
    assert.equal(long.granularity, 'week');
    assert.ok(long.buckets.length <= 180);
});

test('时间轴覆盖整段窗口，缺数据的桶补零且总量守恒', () => {
    const now = new Date(2026, 9, 21, 16, 0, 0).getTime();
    const from = new Date(2026, 9, 21, 12, 0, 0).getTime();
    const points = [
        point(new Date(2026, 9, 21, 12, 5).getTime(), {
            uncachedInputTokens: 10,
            cacheReadTokens: 5,
            outputTokens: 2,
            requestCount: 1,
        }),
        // 同一小时的第二行（后端按状态分行时出现过）：必须并进同一个桶。
        point(new Date(2026, 9, 21, 12, 40).getTime(), {
            uncachedInputTokens: 1,
            requestCount: 2,
        }),
    ];
    const { buckets } = trendBuckets(points, 'hour', from, now);
    assert.equal(buckets.length, 5, '12 点到 16 点共 5 个桶');
    assert.equal(buckets[0].uncachedInput, 11);
    assert.equal(buckets[0].cacheRead, 5);
    assert.equal(buckets[0].requests, 3);
    assert.equal(buckets[0].totalTokens, 18, '总量是五类构成之和');
    assert.equal(buckets[1].totalTokens, 0, '没有数据的小时是空桶');
    assert.equal(
        buckets.reduce((sum, bucket) => sum + bucket.requests, 0),
        3,
        '并桶不丢也不重复计数',
    );
});

test('窗口之外的点被丢弃，不把别的周期画进这一屏', () => {
    const now = new Date(2026, 9, 21, 16, 0, 0).getTime();
    const points = [point(now - 10 * DAY, { uncachedInputTokens: 999 })];
    const { buckets } = trendBuckets(points, 'day', now - 2 * DAY, now);
    assert.equal(
        buckets.reduce((sum, bucket) => sum + bucket.uncachedInput, 0),
        0,
    );
});

test('堆叠段首尾相接，柱高恒等于总量', () => {
    const now = new Date(2026, 9, 21, 16, 0, 0).getTime();
    const { buckets } = trendBuckets(
        [
            point(now, {
                uncachedInputTokens: 10,
                cacheReadTokens: 20,
                cacheWriteTokens: 5,
                outputTokens: 3,
                reasoningTokens: 2,
            }),
        ],
        'hour',
        now,
        now,
    );
    const [segments] = stackColumns(buckets);
    assert.equal(segments[0].from, 0);
    assert.equal(segments[segments.length - 1].to, 40);
    for (let index = 1; index < segments.length; index += 1) {
        assert.equal(segments[index].from, segments[index - 1].to, '段与段之间不留缝');
    }
    const totals = classTotals(buckets);
    assert.equal(
        totals.reduce((sum, item) => sum + item.total, 0),
        40,
    );
});

test('坐标轴分度从 0 起算并覆盖最大值', () => {
    assert.deepEqual(niceTicks(0), [0]);
    const ticks = niceTicks(51_168);
    assert.equal(ticks[0], 0);
    assert.ok(ticks[ticks.length - 1] >= 51_168);
    const step = ticks[1] - ticks[0];
    for (let index = 1; index < ticks.length; index += 1) {
        assert.equal(ticks[index] - ticks[index - 1], step, '分度必须等距');
    }
});

test('日历按周分列：列是周、行是周日到周六，未来日期单独标记', () => {
    const now = new Date(2026, 9, 21, 16, 0, 0).getTime();
    const weeks = calendarWeeks(
        [
            { date: '2026-10-21', level: 4, value: 100 },
            { date: '2026-10-19', level: 1, value: 10 },
        ],
        3,
        now,
    );
    assert.equal(weeks.length, 3);
    for (const column of weeks) assert.equal(column.length, 7);

    const lastColumn = weeks[weeks.length - 1];
    const todayCell = lastColumn.find((cell) => cell.date === '2026-10-21');
    assert.equal(todayCell.level, 4);
    assert.equal(todayCell.value, 100);
    assert.equal(todayCell.future, false);

    const missing = weeks[0][0];
    assert.equal(missing.level, 0, '没有数据的日期补 0 档');
    assert.equal(missing.value, 0);

    const futureCells = weeks.flat().filter((cell) => cell.future);
    assert.ok(futureCells.length > 0, '今天之后的格子必须标成未来');
    assert.ok(futureCells.every((cell) => cell.date > '2026-10-21'));
});

test('健康网格行是天、列是小时，缺数据的小时留空格', () => {
    const now = new Date(2026, 9, 21, 16, 0, 0).getTime();
    const firstHour = new Date(2026, 9, 21, 9, 0, 0).getTime();
    const rows = healthRows(
        [
            { timestampMs: firstHour, status: 'healthy', successRate: 1 },
            { timestampMs: firstHour + HOUR, status: 'degraded', successRate: 0.5 },
        ],
        new Date(2026, 9, 21, 0, 0, 0).getTime(),
        now,
    );
    assert.equal(rows.length, 1);
    assert.equal(rows[0].cells.length, 24);
    assert.equal(rows[0].cells[9].status, 'healthy');
    assert.equal(rows[0].cells[10].status, 'degraded');
    assert.equal(rows[0].cells[3].status, null, '没有请求的小时是空格');
});

test('同一小时出现多行健康结论时取更差的一档', () => {
    const now = new Date(2026, 9, 21, 16, 0, 0).getTime();
    const hour = new Date(2026, 9, 21, 9, 0, 0).getTime();
    const rows = healthRows(
        [
            { timestampMs: hour, status: 'healthy', successRate: 1 },
            { timestampMs: hour, status: 'failed', successRate: 0 },
        ],
        new Date(2026, 9, 21, 0, 0, 0).getTime(),
        now,
    );
    assert.equal(rows[0].cells[9].status, 'failed');
});

test('健康网格最多回看十四天', () => {
    const now = new Date(2026, 9, 21, 16, 0, 0).getTime();
    const rows = healthRows([], now - 60 * DAY, now);
    assert.equal(rows.length, 14);
});

test('分位阶梯四个分位成行，缺观测保留空位', () => {
    const rows = ladderRows({
        p50TtftMs: 100,
        p90TtftMs: 200,
        p95TtftMs: null,
        p99TtftMs: 400,
        p50LatencyMs: 1000,
        p90LatencyMs: 2000,
        p95LatencyMs: null,
        p99LatencyMs: 4000,
    });
    assert.deepEqual(
        rows.map((row) => row.label),
        ['p50', 'p90', 'p95', 'p99'],
    );
    assert.equal(rows[2].ttftMs, null, '没有观测的分位不补零');
    assert.equal(rows[3].latencyMs, 4000);
});

test('散点跳过零值样本，并把跳过数如实交回', () => {
    const geometry = scatterGeometry([
        { ttftMs: 100, totalDurationMs: 1000, tokens: 10 },
        { ttftMs: 0, totalDurationMs: 0, tokens: 0 },
        { ttftMs: 200, totalDurationMs: 4000, tokens: 1000 },
    ]);
    assert.equal(geometry.dots.length, 2);
    assert.equal(geometry.skipped, 1);
    const [small, large] = geometry.dots;
    assert.ok(large.radius > small.radius, '点半径随 Token 体量增长');
    assert.ok(geometry.dots.every((dot) => dot.x >= 0 && dot.x <= 1));
    assert.ok(geometry.dots.every((dot) => dot.y >= 0 && dot.y <= 1));
    assert.equal(scatterGeometry([]).dots.length, 0);
});
