import test from 'node:test';
import assert from 'node:assert/strict';
import { turnChannelLabel } from '../src/conversation/channelLabel.ts';
import { historySource } from '../src/conversation/historyProjection.ts';

/*
 * 逐轮来源标注（D12）：同一段会话可能由桌面、TUI 与 CLI 先后续聊，
 * 来源必须按轮给出，不能取会话级统一值。
 */

test('已知渠道给出界面标签', () => {
    assert.equal(turnChannelLabel('cli'), 'CLI');
    assert.equal(turnChannelLabel('desktop'), '桌面');
    assert.equal(turnChannelLabel('tui'), 'TUI');
    assert.equal(turnChannelLabel('cron'), '定时');
    assert.equal(turnChannelLabel('webhook'), 'Webhook');
    assert.equal(turnChannelLabel('goal'), '目标执行');
});

test('逐轮来源客户端优先，缺省才回落到车道渠道', () => {
    // 桌面车道上的一轮由 CLI 续聊：来源是 CLI，不是桌面。
    assert.equal(historySource({ channel: 'desktop', client: 'cli' }), 'cli');
    assert.equal(turnChannelLabel(historySource({ channel: 'desktop', client: 'cli' })), 'CLI');
    // 没有客户端事实的轮次（外部事件、定时唤醒）退回渠道本身。
    assert.equal(historySource({ channel: 'tui' }), 'tui');
    assert.equal(historySource({ channel: 'jira', client: '' }), 'jira');
    assert.equal(turnChannelLabel(historySource({ channel: 'jira', client: '' })), 'jira');
});

test('同一会话三轮来自不同入口时各标各的', () => {
    const turns = [
        { channel: 'desktop', client: 'desktop' },
        { channel: 'desktop', client: 'cli' },
        { channel: 'desktop', client: 'cron' },
    ];
    const labels = turns.map((turn) => turnChannelLabel(historySource(turn)));
    assert.deepEqual(labels, ['桌面', 'CLI', '定时']);
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
