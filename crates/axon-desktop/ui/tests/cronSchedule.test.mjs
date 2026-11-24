import test from 'node:test';
import assert from 'node:assert/strict';
import { encodeSchedule, parseSchedule, scheduleError, scheduleText } from '../src/components/automation/schedule.ts';

/*
 * 计划文案与表单互转的回归网。
 *
 * 两条不变量：
 * 1. 语法合法的表达式都要读成中文，哪怕控件编辑不了；
 * 2. 控件能编辑的表达式，回填后原样还原，不能悄悄改掉触发频率。
 */

test('固定间隔收成人读得懂的单位', () => {
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
    assert.equal(scheduleText('0 0 1 1 *'), '每年 1 月 1 日 00:00');
    assert.equal(scheduleText('workday:0 8-20 * * *'), '每个工作日 8 点到 20 点每小时');
    assert.equal(scheduleText('workday:0 8-20/2 * * *'), '每个工作日 8 点到 20 点每 2 小时');
    assert.equal(scheduleText('workday:*/30 9-18 * * *'), '每个工作日 9 点到 18 点每 30 分钟');
    assert.equal(scheduleText('30 9-18 * * *'), '每天 9 点到 18 点每小时的第 30 分');
});

test('日历范围与日期方式自由组合', () => {
    assert.equal(scheduleText('workday:0 9 * * 5'), '每个工作日里的周五 09:00');
    assert.equal(scheduleText('holiday:0 8 * * *'), '每个法定节假日 08:00');
    assert.equal(scheduleText('holiday:0 8 1 * *'), '每个法定节假日里的 1 日 08:00');
    assert.equal(scheduleText('weekend:0 10 * * 6'), '每个周末里的周六 10:00');
    assert.equal(scheduleText('workday:0 9 1 1 *'), '每个工作日里的 1 月 1 日 09:00');
    assert.equal(scheduleText('weekday:0 9 * * 1-5'), '周一到周五 09:00', '星期等于工作日集合时不重复');
});

test('连续与等差的小时列表归一化为时段', () => {
    const contiguous = '0 9,10,11,12,13,14,15,16,17,18,19,20 * * *';
    assert.equal(scheduleText(contiguous), '每天 9 点到 20 点每小时');
    assert.equal(encodeSchedule(parseSchedule(contiguous)), '0 9-20 * * *');

    const stepped = '0 9,12,15,18,21 * * *';
    assert.equal(scheduleText(stepped), '每天 9 点到 21 点每 3 小时');
    assert.equal(encodeSchedule(parseSchedule(stepped)), '0 9-21/3 * * *');
    assert.equal(parseSchedule('0 9,12,14 * * *').kind, 'custom', '非等差仍回落自定义');
});

test('控件编辑不了但合法的计划同样读成中文', () => {
    assert.equal(scheduleText('* * * * *'), '每分钟');
    assert.equal(scheduleText('0 * * * *'), '每小时的第 0 分');
    assert.equal(scheduleText('0,30 * * * *'), '每小时的第 0、30 分');
    assert.equal(scheduleText('0 */2 * * *'), '0 点到 22 点每 2 小时');
    assert.equal(scheduleText('0 0 1,15 * *'), '每月 1、15 日 00:00');
    assert.equal(scheduleText('0 9 1 * 1'), '每月 1 日或每周一 09:00');
});

test('一次执行按全站时刻口径展示', () => {
    assert.equal(scheduleText('once:2099-12-31T23:30:00'), '2099年12月31日 23:30 执行一次');
});

test('表单回填与还原对同一表达式收敛', () => {
    const expectations = [
        ['workday:0 8-20 * * *', 'workday', 'none'],
        ['workday:0 9 * * 5', 'workday', 'weekly'],
        ['holiday:0 8 1 * *', 'holiday', 'monthly'],
        ['workday:0 9 1 1 *', 'workday', 'yearly'],
        ['workday:*/30 9-18 * * *', 'workday', 'none'],
        ['holiday:0 10 * * *', 'holiday', 'none'],
        ['0 20 * * 1,2,3,4,5', '', 'weekly'],
        ['0 9 * * *', '', 'none'],
        ['0 9-18 * * *', '', 'none'],
        ['30 9-18 * * *', '', 'none'],
        ['30 9 * * *', '', 'none'],
        ['0 0 1 * *', '', 'monthly'],
        ['0 0 1 1 *', '', 'yearly'],
        ['interval:30m', '', 'none'],
    ];
    for (const [expression, calendar, dateMode] of expectations) {
        const form = parseSchedule(expression);
        assert.notEqual(form.kind, 'custom', expression);
        assert.equal(form.calendar, calendar, expression);
        assert.equal(form.dateMode, dateMode, expression);
        assert.equal(encodeSchedule(form), expression, expression);
    }
});

test('控件表达不了的计划回落自定义并原样保留', () => {
    for (const expression of [
        '* * * * *',
        '0 * * * *',
        '*/5 * * * *',
        '*/10 8-20/2 * * *',
        '0 9/2 * * *',
        '0 9 1 * 1',
        '0 9 1,15 1 *',
        'holiday:0 8 1,15 * *',
        'workday:0 8 * 1 *',
        '0 9-18/25 * * *',
        'interval:2d',
    ]) {
        const form = parseSchedule(expression);
        assert.equal(form.kind, 'custom', expression);
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
