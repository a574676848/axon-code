import test from 'node:test';
import assert from 'node:assert/strict';
import { cronTimeText, scheduleText } from '../src/components/automation/schedule.ts';

/*
 * Cron 展示口径：周期文案与时间文案。
 *
 * 自动化任务页与「设置 > 模型定价」的自动同步状态行共用这两个函数，
 * 因此它们的边界要在这里钉住：两处读同一批任务，说法必须一致。
 */

test('周期文案把毫秒间隔收成人读得懂的单位', () => {
    assert.equal(scheduleText('interval:86400000ms'), '每 1 天');
    assert.equal(scheduleText('interval:60m'), '每 1 小时');
    assert.equal(scheduleText('interval:60s'), '每 1 分钟');
    assert.equal(scheduleText('interval:90m'), '每 90 分钟', '只在能整除时才用更大的单位');
    assert.equal(scheduleText('interval:1500ms'), '每 1.5 秒', '不足一秒的余数写成分秒');
    assert.equal(scheduleText('interval:1ms'), '每 1 毫秒');
    assert.equal(scheduleText('0 9 * * *'), '每天 09:00');
});

test('时间文案是本地完整时刻，没有安排时写未安排', () => {
    const at = new Date(2026, 9, 22, 17, 14, 17).getTime();
    const text = cronTimeText(at);
    assert.match(text, /^2026\/10\/22/, `本地完整日期开头: ${text}`);
    assert.match(text, /17:14:17$/, `读到时秒: ${text}`);
    assert.equal(cronTimeText(null), '未安排');
    assert.equal(cronTimeText(undefined), '未安排');
});
