import test from 'node:test';
import assert from 'node:assert/strict';
import { turnChannelLabel } from '../src/conversation/channelLabel.ts';

/*
 * 逐轮来源标注（D12）：同一段会话可能由桌面、TUI 与 CLI 先后续聊，
 * 来源必须按轮给出，不能取会话级统一值。
 */

test('已知渠道给出界面标签', () => {
    assert.equal(turnChannelLabel('cli'), 'CLI');
    assert.equal(turnChannelLabel('desktop'), '桌面');
    assert.equal(turnChannelLabel('tui'), 'TUI');
    assert.equal(turnChannelLabel('cron'), '定时');
    assert.equal(turnChannelLabel('goal'), '目标执行');
});

test('未知渠道原样给出，不猜成某一端', () => {
    assert.equal(turnChannelLabel('wecom'), 'wecom');
});

test('没有渠道事实时不给标签，也不写占位', () => {
    assert.equal(turnChannelLabel(''), '');
    assert.equal(turnChannelLabel(undefined), '');
    assert.equal(turnChannelLabel(null), '');
    assert.equal(turnChannelLabel('   '), '');
});
