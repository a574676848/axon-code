import test from 'node:test';
import assert from 'node:assert/strict';

import {
    normalizePathForCompare,
    resolveRestoreTarget,
    workspaceOfDir,
} from '../src/restoreView.ts';

function workspace(id, path, sessions, active = false) {
    return { id, name: id, path, active, removable: true, sessions };
}

function session(id, updatedAt, extra = {}) {
    return {
        id,
        title: id,
        channel: 'desktop',
        destination: `workbench-${id}`,
        audience: '私密',
        interaction_count: 1,
        pending_question_count: 0,
        created_at: 1,
        updated_at: updatedAt,
        pinned: false,
        archived: false,
        pending: false,
        ...extra,
    };
}

test('path compare normalizes windows separators, extended prefixes and case', () => {
    assert.equal(normalizePathForCompare('C:\\Users\\Me\\.axon\\'), 'c:/users/me/.axon');
    assert.equal(normalizePathForCompare('//?/c:/users/me/.axon'), 'c:/users/me/.axon');
    assert.equal(normalizePathForCompare('  /home/me/repo/  '), '/home/me/repo');
    assert.equal(normalizePathForCompare(''), '');
});

test('workspace lookup matches the recorded directory across spellings', () => {
    const workspaces = [
        workspace('ws-a', '\\\\?\\C:\\Users\\Me\\.axon', []),
        workspace('ws-b', 'E:\\zbg\\axon-code', []),
    ];
    assert.equal(workspaceOfDir(workspaces, 'C:/Users/Me/.axon')?.id, 'ws-a');
    assert.equal(workspaceOfDir(workspaces, 'e:\\zbg\\axon-code\\')?.id, 'ws-b');
    assert.equal(workspaceOfDir(workspaces, 'D:/nowhere'), null);
    assert.equal(workspaceOfDir(workspaces, null), null);
});

test('restore prefers the recorded workspace and its most recent session', () => {
    const workspaces = [
        workspace('ws-home', 'C:/Users/Me/.axon', [
            session('older', 100),
            session('newest', 300),
        ]),
        workspace('ws-repo', 'E:/repo', [session('repo-session', 900)], true),
    ];
    const target = resolveRestoreTarget(workspaces, { workspaceDir: 'C:/Users/Me/.axon' });
    assert.equal(target?.workspace.id, 'ws-home', '目录命中优先于 active 标记');
    assert.equal(target?.session.id, 'newest', '没有记录会话时取该工作区最近活动的一条');
});

test('recorded session id wins even when it is not the most recent', () => {
    const workspaces = [
        workspace('ws-home', 'C:/Users/Me/.axon', [session('older', 100), session('newest', 300)]),
    ];
    const target = resolveRestoreTarget(workspaces, {
        workspaceDir: 'C:/Users/Me/.axon',
        sessionId: 'older',
    });
    assert.equal(target?.session.id, 'older');
});

test('unknown directory falls back to the workspace owning the recorded session', () => {
    const workspaces = [
        workspace('ws-home', 'C:/Users/Me/.axon', [session('a', 100)]),
        workspace('ws-repo', 'E:/repo', [session('repo-session', 50)]),
    ];
    const target = resolveRestoreTarget(workspaces, {
        workspaceDir: 'D:/gone',
        sessionId: 'repo-session',
    });
    assert.equal(target?.workspace.id, 'ws-repo');
    assert.equal(target?.session.id, 'repo-session');
});

test('archived sessions are never opened by the fallback', () => {
    const workspaces = [
        workspace('ws-home', 'C:/Users/Me/.axon', [
            session('gone', 900, { archived: true }),
            session('live', 100),
        ]),
    ];
    const target = resolveRestoreTarget(workspaces, { workspaceDir: 'C:/Users/Me/.axon' });
    assert.equal(target?.session.id, 'live');
});

test('no workspace or no session leaves the hero screen untouched', () => {
    assert.equal(resolveRestoreTarget([], { workspaceDir: 'C:/Users/Me/.axon' }), null);
    assert.equal(
        resolveRestoreTarget([workspace('ws-empty', 'C:/x', [])], { workspaceDir: 'C:/x' }),
        null,
    );
});

test('a local draft is never restored: it has no content to return to', () => {
    const workspaces = [
        workspace('ws-home', 'C:/Users/Me/.axon', [
            session('draft-a', 900, { draft: true, pending: true, interaction_count: 0 }),
            session('live', 100),
        ]),
    ];
    const target = resolveRestoreTarget(workspaces, { workspaceDir: 'C:/Users/Me/.axon' });
    assert.equal(target?.session.id, 'live', '草稿比 live 更新，但它不是「上次看到哪」');
});

test('a session without a single turn is never restored', () => {
    const workspaces = [
        workspace('ws-home', 'C:/Users/Me/.axon', [
            session('empty', 900, { interaction_count: 0 }),
            session('live', 100),
        ]),
    ];
    const target = resolveRestoreTarget(workspaces, { workspaceDir: 'C:/Users/Me/.axon' });
    assert.equal(target?.session.id, 'live', '零轮会话被自动选中才会看起来像刚被创建出来');
});

test('a recorded session id pointing at an empty session falls back to real content', () => {
    const workspaces = [
        workspace('ws-home', 'C:/Users/Me/.axon', [
            session('ghost', 900, { interaction_count: 0 }),
            session('live', 100),
        ]),
    ];
    const target = resolveRestoreTarget(workspaces, {
        workspaceDir: 'C:/Users/Me/.axon',
        sessionId: 'ghost',
    });
    assert.equal(target?.session.id, 'live', '记录里的标识失效时退回有内容的最近一条');
});

test('a workspace holding only empty sessions stays on the hero screen', () => {
    const workspaces = [
        workspace('ws-home', 'C:/Users/Me/.axon', [
            session('ghost', 900, { interaction_count: 0 }),
            session('draft-b', 950, { draft: true, pending: true, interaction_count: 0 }),
        ]),
    ];
    assert.equal(
        resolveRestoreTarget(workspaces, { workspaceDir: 'C:/Users/Me/.axon', sessionId: 'ghost' }),
        null,
        '没有任何有内容的会话时停在首屏，不硬造一条',
    );
});
