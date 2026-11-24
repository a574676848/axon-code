import test from 'node:test';
import assert from 'node:assert/strict';

import { shouldLeaveSession } from '../src/sessionPresence.ts';

const seen = (archived = false) => ({ sessionId: 'session-1', archived });

test('同一条会话从清单里消失（删除）要退回首屏', () => {
    assert.equal(shouldLeaveSession(seen(), 'session-1', null), true);
});

test('同一条会话由未归档翻成已归档要退回首屏', () => {
    assert.equal(shouldLeaveSession(seen(), 'session-1', { archived: true }), true);
});

test('打开一条本来就在归档夹里的会话不算归档，不许弹走', () => {
    assert.equal(shouldLeaveSession(seen(true), 'session-1', { archived: true }), false);
});

test('还没观察过这一条时不下结论：目标与子代理会话不进侧栏清单', () => {
    assert.equal(shouldLeaveSession(null, 'goal-session', null), false);
    assert.equal(shouldLeaveSession(seen(), 'another-session', null), false);
});

test('形态没变时停在原处', () => {
    assert.equal(shouldLeaveSession(seen(), 'session-1', { archived: false }), false);
});
