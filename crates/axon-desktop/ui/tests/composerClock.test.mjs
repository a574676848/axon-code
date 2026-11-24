import test from 'node:test';
import assert from 'node:assert/strict';
import { formatElapsedClock } from '../src/conversation/format.ts';

/*
 * 输入坞上沿两条状态条共用同一条计时读数：分:秒，满一小时补上小时位。
 * 进位边界是这一份格式唯一的风险点，因此逐个钉住。
 */

test('计时读数按分:秒给出，秒恒两位', () => {
    assert.equal(formatElapsedClock(0), '0:00');
    assert.equal(formatElapsedClock(9_400), '0:09', '不足十秒也占两位，数字变化时不跳宽');
    assert.equal(formatElapsedClock(59_999), '0:59');
    assert.equal(formatElapsedClock(62_000), '1:02');
});

test('跨过整分钟与整小时都正确进位，不出现 0:60 与 60:00', () => {
    assert.equal(formatElapsedClock(60_000), '1:00');
    assert.equal(formatElapsedClock(3_599_999), '59:59');
    assert.equal(formatElapsedClock(3_600_000), '1:00:00');
    assert.equal(formatElapsedClock(3_723_000), '1:02:03');
});

test('负数与零输入收敛到零点，不产出负号读数', () => {
    assert.equal(formatElapsedClock(-500), '0:00');
});
