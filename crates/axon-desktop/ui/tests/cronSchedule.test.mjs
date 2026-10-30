import test from 'node:test';
import assert from 'node:assert/strict';
import { encodeSchedule, parseSchedule, scheduleError, scheduleText } from '../src/components/automation/schedule.ts';

/*
 * Cron 周期文案：自动化任务页与「设置 > 模型定价」的自动同步状态行共用这一个函数，
 * 因此它的边界要在这里钉住：两处读同一批任务，说法必须一致。
 *
 * 解析器只对能被表单无损重建的表达式给出结构化结果，其余回落 custom 并保留原文；
 * 下面的回落用例就是这条约束的回归网，任何一条被改写成等价但不同的表达式都算缺陷。
 */

test('周期文案把毫秒间隔收成人读得懂的单位', () => {
    assert.equal(scheduleText('interval:86400000ms'), '每 1 天');
    assert.equal(scheduleText('interval:60m'), '每 1 小时');
    assert.equal(scheduleText('interval:60s'), '每 1 分钟');
    assert.equal(scheduleText('interval:90m'), '每 90 分钟', '只在能整除时才用更大的单位');
    assert.equal(scheduleText('interval:1500ms'), '每 1.5 秒');
    assert.equal(scheduleText('interval:1ms'), '每 1 毫秒');
});

test('常见计划读成中文', () => {
    assert.equal(scheduleText('0 9 * * *'), '每天 09:00');
    assert.equal(scheduleText('0 20 * * 1,2,3,4,5'), '每个工作日 20:00');
    assert.equal(scheduleText('0 21 * * 5'), '每周五 21:00');
    assert.equal(scheduleText('0 0 1 * *'), '每月 1 日 00:00');
    assert.equal(scheduleText('workday:0 8-20 * * *'), '每个工作日 8 点到 20 点每小时');
    assert.equal(scheduleText('workday:0 8-20/2 * * *'), '每个工作日 8 点到 20 点每 2 小时');
    assert.equal(scheduleText('workday:*/30 9-18 * * *'), '每个工作日 9 点到 18 点每 30 分钟');
    assert.equal(scheduleText('30 9-18 * * *'), '每天 9 点到 18 点每小时的第 30 分');
    assert.equal(scheduleText('weekday:0 9 * * *'), '周一到周五 09:00');
    assert.equal(scheduleText('holiday:0 10 * * *'), '每个法定节假日 10:00');
    assert.equal(scheduleText('weekend:0 9 * * *'), '每个周末 09:00');
});

test('连续小时逗号列表归一化为时段', () => {
    const expression = '0 9,10,11,12,13,14,15,16,17,18,19,20 * * *';
    const form = parseSchedule(expression);
    assert.equal(form.mode, 'daily');
    assert.equal(form.span, true);
    assert.equal(scheduleText(expression), '每天 9 点到 20 点每小时');
    assert.equal(encodeSchedule(form), '0 9-20 * * *');
});

test('等差的小时逗号列表归一化为区间加步长', () => {
    const expression = '0 9,12,15,18,21 * * *';
    const form = parseSchedule(expression);
    assert.equal(form.mode, 'daily');
    assert.equal(form.span, true);
    assert.equal(form.spanEvery, '3');
    assert.equal(scheduleText(expression), '每天 9 点到 21 点每 3 小时');
    assert.equal(encodeSchedule(form), '0 9-21/3 * * *');
    assert.equal(parseSchedule('0 9,12,14 * * *').mode, 'custom', '非等差仍回落自定义');
});

test('不可编辑但合法的计划同样读成中文', () => {
    assert.equal(scheduleText('* * * * *'), '每分钟');
    assert.equal(scheduleText('0,30 * * * *'), '每小时的第 0、30 分');
    assert.equal(scheduleText('0 */2 * * *'), '0 点到 22 点每 2 小时');
    assert.equal(scheduleText('0 0 1,15 * *'), '每月 1、15 日 00:00');
    assert.equal(scheduleText('0 0 1 1 *'), '每年 1 月 1 日 00:00');
    assert.equal(scheduleText('0 9 1 * 1'), '每月 1 日或每周一 09:00');
    assert.equal(scheduleText('0 9 * * 1,3,5'), '每周一、周三、周五 09:00');
});

test('一次执行按全站时刻口径展示', () => {
    assert.equal(scheduleText('once:2099-12-31T23:30:00'), '2099年12月31日 23:30 执行一次');
});

test('表单回填与还原对同一表达式收敛', () => {
    const expectations = [
        ['workday:0 8-20 * * *', 'workday'],
        ['workday:*/30 9-18 * * *', 'workday'],
        ['holiday:0 10 * * *', 'holiday'],
        ['0 20 * * 1,2,3,4,5', 'weekly'],
        ['0 9 * * *', 'daily'],
        ['0 9-18 * * *', 'daily'],
        ['30 9-18 * * *', 'daily'],
        ['30 9 * * *', 'daily'],
        ['0 0 1 * *', 'monthly'],
        ['interval:30m', 'interval'],
    ];
    for (const [expression, mode] of expectations) {
        const form = parseSchedule(expression);
        assert.notEqual(form.mode, 'custom', expression);
        assert.equal(form.mode, mode, expression);
        assert.equal(encodeSchedule(form), expression, expression);
    }
});

test('表单表达不了的计划回落自定义并原样保留', () => {
    for (const expression of [
        '* * * * *',
        '0 * * * *',
        '*/5 * * * *',
        '*/10 8-20/2 * * *',
        '0 9/2 * * *',
        '0 9 1 * 1',
        '0 9 1,15 1 *',
        '0 9-18/25 * * *',
        'workday:0 9 1 * *',
        'weekend:0 10 * * 6',
        'interval:2d',
    ]) {
        const form = parseSchedule(expression);
        assert.equal(form.mode, 'custom', expression);
        assert.equal(encodeSchedule(form), expression, expression);
    }
});

test('计划校验拦住倒置时段与越界步长', () => {
    const span = parseSchedule('workday:0 9-18 * * *');
    assert.equal(scheduleError(span), null);
    assert.equal(scheduleError({ ...span, spanStart: '20', spanEnd: '08' }), '时段起始小时不能晚于结束小时');
    assert.equal(scheduleError({ ...span, spanEvery: '0' }), '时段间隔必须是大于零的整数');
    assert.equal(scheduleError({ ...span, spanEvery: '60', spanUnit: 'minute' }), '分钟间隔不能超过 59');
    assert.equal(scheduleError({ ...span, spanEvery: '25', spanUnit: 'hour' }), '小时间隔不能超过 24');
    assert.equal(scheduleError(parseSchedule('once:2099-12-31T23:30:00')), null);
    assert.equal(scheduleError(parseSchedule('once:2020-01-01T00:00:00')), '一次执行的日期和时间必须晚于当前时间');
    assert.equal(scheduleError({ ...parseSchedule('interval:30m'), amount: '0' }), '间隔必须是大于零的整数');
});
