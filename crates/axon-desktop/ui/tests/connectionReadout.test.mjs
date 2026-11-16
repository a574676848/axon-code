import test from 'node:test';
import assert from 'node:assert/strict';
import { connectionSummary, readoutText } from '../src/connectionReadout.ts';

/** 健康结论夹具。 */
function health(dot, label, detail = '') {
    return { dot, label, detail };
}

test('健康档写延迟，不写状态词', () => {
    assert.equal(readoutText(health('done', '已连接'), '7 毫秒'), '7 毫秒');
});

test('健康但没取到延迟时退回健康结论，不写 0', () => {
    assert.equal(readoutText(health('done', '已连接'), null), '已连接');
});

test('后台已停机这类降级档一律写结论，不写延迟', () => {
    const degraded = health('warning', '后台已停机', '网关有响应，但后台租约已失效');
    assert.equal(readoutText(degraded, '3 毫秒'), '后台已停机', '延迟再低也不代表后台在干活');
    assert.equal(readoutText(degraded, null), '后台已停机');
    assert.equal(readoutText(health('error', '连不上'), '2 毫秒'), '连不上');
});

test('快照还没取到时不编造读数', () => {
    assert.equal(readoutText(null, '7 毫秒'), null);
});

test('整行结论：健康补延迟，降级补可执行说明', () => {
    assert.equal(
        connectionSummary('本机', health('done', '已连接'), '7 毫秒'),
        '本机，已连接，延迟 7 毫秒',
    );
    assert.equal(
        connectionSummary(
            '本机',
            health('warning', '后台已停机', '重启后台服务即可恢复'),
            '3 毫秒',
        ),
        '本机，后台已停机，重启后台服务即可恢复',
        '降级时不写延迟：那句话会把它读成慢，而实际是停了',
    );
});

test('档案名与结论同值时只说一遍', () => {
    assert.equal(connectionSummary('未连接', health('idle', '未连接', ''), null), '未连接');
});
