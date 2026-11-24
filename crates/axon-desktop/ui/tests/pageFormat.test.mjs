import test from 'node:test';
import assert from 'node:assert/strict';
import {
    formatDateTime,
    formatDuration,
    formatMoment,
    formatPercent,
} from '../src/components/pageFormat.ts';

/*
 * 非对话页共用的时刻口径（ui-constraints §3.2）：同年 `10/21 17:14`，跨年
 * `2026年10月21日 17:14`，界面不显示秒。自动化任务、记忆轮盘与模型定价三处
 * 说的都是同一批时刻，因此这两档边界在这里钉死。
 */

const NOW = new Date(2026, 9, 23, 14, 5, 0).getTime();

test('同年只写月日与时分，不重复年份也不显示秒', () => {
    const at = new Date(2026, 9, 22, 17, 14, 17).getTime();
    assert.equal(formatDateTime(at, NOW), '10/22 17:14');
    assert.equal(formatDateTime(new Date(2026, 0, 5, 9, 5, 59).getTime(), NOW), '1/5 09:05');
});

test('跨年补年份，避免与同年日期混淆', () => {
    const at = new Date(2025, 11, 31, 23, 59, 30).getTime();
    assert.equal(formatDateTime(at, NOW), '2025年12月31日 23:59');
    const next = new Date(2027, 0, 1, 0, 0, 5).getTime();
    assert.equal(formatDateTime(next, NOW), '2027年1月1日 00:00');
});

test('缺值的时刻写未安排，不落成 1970 年', () => {
    assert.equal(formatMoment(null, NOW), '未安排');
    assert.equal(formatMoment(undefined, NOW), '未安排');
    assert.equal(formatMoment(new Date(2026, 9, 22, 17, 14).getTime(), NOW), '10/22 17:14');
});

test('时长分档：不足一分钟带一位小数，超过分钟收成分秒', () => {
    assert.equal(formatDuration(300), '0.3 秒');
    assert.equal(formatDuration(66_000), '1分6秒');
    assert.equal(formatDuration(3_600_000), '1小时0分');
    assert.equal(formatDuration(3_665_000), '1小时1分5秒');
});

test('百分比只保留整数', () => {
    assert.equal(formatPercent(1234), '12%');
    assert.equal(formatPercent(0), '0%');
    assert.equal(formatPercent(null), '0%');
});
