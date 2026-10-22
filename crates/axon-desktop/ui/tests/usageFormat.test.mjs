import test from 'node:test';
import assert from 'node:assert/strict';
import {
    costLabel,
    formatCostMicros,
    formatCount,
    formatLatency,
    formatPercent,
    formatRelative,
    formatTokens,
    sourceLabel,
    statusLabel,
    statusTone,
} from '../src/components/usage/usageFormat.ts';

test('Token 体量按量级取单位，不足一千不出现小数', () => {
    assert.equal(formatTokens(0), '0');
    assert.equal(formatTokens(999), '999');
    assert.equal(formatTokens(51_168), '51.17K');
    assert.equal(formatTokens(27_452_408), '27.45M');
    assert.equal(formatTokens(2_500_000_000), '2.50B');
});

test('计数带千分位，百分比由万分比换算', () => {
    assert.equal(formatCount(28), '28');
    assert.equal(formatCount(1234), '1,234');
    assert.equal(formatPercent(8789), '87.89%');
    assert.equal(formatPercent(8789, 0), '88%');
});

test('延迟按量级切单位，缺观测如实写无观测', () => {
    assert.equal(formatLatency(820), '820ms');
    assert.equal(formatLatency(3353), '3.35s');
    assert.equal(formatLatency(257_916), '4分18秒');
    assert.equal(formatLatency(null), '无观测');
    assert.equal(formatLatency(0), '无观测');
});

test('成本保留四位小数，未定价单独成句', () => {
    assert.equal(formatCostMicros(15_900), '$0.0159');
    assert.equal(formatCostMicros(0), '$0.00');
    assert.equal(costLabel(15_900, 3), '$0.0159');
    assert.equal(costLabel(0, 3), '未定价');
    assert.equal(costLabel(0, 0), '无请求');
});

test('相对时间按分钟与小时分档', () => {
    const now = 1_000_000_000;
    assert.equal(formatRelative(now - 3_000, now), '刚刚');
    assert.equal(formatRelative(now - 45_000, now), '45 秒前');
    assert.equal(formatRelative(now - 5 * 60_000, now), '5 分钟前');
    assert.equal(formatRelative(now - 3 * 3_600_000, now), '3 小时前');
});

test('来源与终态按界面口径翻译，未知取值原样透出', () => {
    assert.equal(sourceLabel('axon_cli'), '主 Agent');
    assert.equal(sourceLabel('subagent'), '子代理');
    assert.equal(sourceLabel('other'), 'other');
    assert.equal(statusLabel('completed'), '成功');
    assert.equal(statusLabel('interrupted'), '中断');
    assert.equal(statusLabel('weird'), 'weird');
    assert.equal(statusTone('completed'), 'ok');
    assert.equal(statusTone('failed'), 'danger');
    assert.equal(statusTone('interrupted'), 'warning');
    assert.equal(statusTone('active'), 'idle');
});
