import test from 'node:test';
import assert from 'node:assert/strict';
import { scheduleText } from '../src/components/automation/schedule.ts';

/*
 * Cron 周期文案：自动化任务页与「设置 > 模型定价」的自动同步状态行共用这一个函数，
 * 因此它的边界要在这里钉住：两处读同一批任务，说法必须一致。
 *
 * 时刻文案归 `pageFormat.formatMoment` 管（同年 `10/21 17:14`，跨年带年份），
 * 边界见 `pageFormat.test.mjs`。
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
