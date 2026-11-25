import test from 'node:test';
import assert from 'node:assert/strict';
import {
    SPACE_HOLD_MS,
    canStartSpaceHold,
    resolveSpaceHold,
    spaceHoldProgress,
    shouldShowHoldProgress,
} from '../src/speech/spaceHold.ts';

const key = (over = {}) => ({
    code: 'Space',
    repeat: false,
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    isComposing: false,
    ...over,
});

const idle = { hasDraft: false, focusedInText: false };

test('空输入坞的非重复空格才能作为唤醒起点', () => {
    assert.equal(canStartSpaceHold(key(), idle), true);
});

test('打字误触：有草稿或焦点在文本框时不唤醒', () => {
    assert.equal(canStartSpaceHold(key(), { hasDraft: true, focusedInText: false }), false);
    assert.equal(canStartSpaceHold(key(), { hasDraft: false, focusedInText: true }), false);
});

test('组合键、重复键、输入法组合态一律不唤醒', () => {
    assert.equal(canStartSpaceHold(key({ ctrlKey: true }), idle), false);
    assert.equal(canStartSpaceHold(key({ metaKey: true }), idle), false);
    assert.equal(canStartSpaceHold(key({ altKey: true }), idle), false);
    assert.equal(canStartSpaceHold(key({ shiftKey: true }), idle), false);
    assert.equal(canStartSpaceHold(key({ repeat: true }), idle), false);
    assert.equal(canStartSpaceHold(key({ isComposing: true }), idle), false);
});

test('非空格键与被别的监听消费的事件不唤醒', () => {
    assert.equal(canStartSpaceHold(key({ code: 'KeyA' }), idle), false);
    assert.equal(canStartSpaceHold(key({ defaultPrevented: true }), idle), false);
});

test('到 3 秒才算唤醒，不足或被拒绝一律取消', () => {
    assert.equal(resolveSpaceHold(true, SPACE_HOLD_MS), 'triggered');
    assert.equal(resolveSpaceHold(true, SPACE_HOLD_MS + 40), 'triggered');
    assert.equal(resolveSpaceHold(true, SPACE_HOLD_MS - 1), 'cancelled');
    assert.equal(resolveSpaceHold(false, SPACE_HOLD_MS), 'cancelled');
});

test('进度钳在 0 到 1，非法时长按 0 处理', () => {
    assert.equal(spaceHoldProgress(0), 0);
    assert.equal(spaceHoldProgress(-1), 0);
    assert.equal(spaceHoldProgress(Number.NaN), 0);
    assert.equal(spaceHoldProgress(SPACE_HOLD_MS / 2), 0.5);
    assert.equal(spaceHoldProgress(SPACE_HOLD_MS * 2), 1);
});

test('进度只在按住后且未到点之间显示', () => {
    assert.equal(shouldShowHoldProgress(0), false);
    assert.equal(shouldShowHoldProgress(600), true);
    assert.equal(shouldShowHoldProgress(SPACE_HOLD_MS), false);
});
