import test from 'node:test';
import assert from 'node:assert/strict';
import { formatMessageTime } from '../src/conversation/format.ts';

/** 固定「现在」：2026-10-07 周三 14:30 本地时间。 */
const NOW = new Date(2026, 9, 7, 14, 30, 0).getTime();
const at = (...parts) => new Date(...parts).getTime();

test('一分钟内显示刚刚', () => {
    assert.equal(formatMessageTime(NOW - 5_000, NOW), '刚刚');
    assert.equal(formatMessageTime(NOW - 59_000, NOW), '刚刚');
});

test('当天显示时:分', () => {
    assert.equal(formatMessageTime(at(2026, 9, 7, 9, 5), NOW), '09:05');
    assert.equal(formatMessageTime(at(2026, 9, 7, 0, 0), NOW), '00:00');
});

test('前一天显示昨天加时:分，跨零点按日历日判定', () => {
    assert.equal(formatMessageTime(at(2026, 9, 6, 19, 49), NOW), '昨天 19:49');
    // 只差三小时但已跨零点：仍算昨天。
    assert.equal(formatMessageTime(at(2026, 9, 6, 23, 50), at(2026, 9, 7, 0, 20)), '昨天 23:50');
});

test('同年更早显示月日加时:分', () => {
    assert.equal(formatMessageTime(at(2026, 2, 5, 8, 3), NOW), '3月5日 08:03');
    assert.equal(formatMessageTime(at(2026, 0, 1, 0, 0), NOW), '1月1日 00:00');
});

test('跨年显示年月日加时:分', () => {
    assert.equal(formatMessageTime(at(2025, 11, 31, 23, 59), NOW), '2025年12月31日 23:59');
});

test('未来时间不显示刚刚，按当天时:分处理', () => {
    assert.equal(formatMessageTime(NOW + 5_000, NOW), '14:30');
});
