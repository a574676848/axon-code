import test from 'node:test';
import assert from 'node:assert/strict';

import { buildSidebarTree } from '../src/components/sidebarTree.ts';

/*
 * 侧栏的三档归档筛选：归档会话必须能单独筛出来。
 * 归档是展示层筛选，取数一侧读的是含归档的完整清单，因此这里锁定「筛完还剩谁」。
 */

function sessionOf(id, { archived = false, pending = false, updatedAt = 1 } = {}) {
    return {
        id,
        title: id,
        channel: 'desktop',
        destination: `workbench-${id}`,
        audience: '私密',
        interaction_count: pending ? 0 : 3,
        pending_question_count: 0,
        created_at: 1,
        updated_at: updatedAt,
        pinned: false,
        archived,
        pending,
        draft: false,
        running: false,
    };
}

function workspaceOf(id, sessions) {
    return { id, name: id, path: `E:/${id}`, active: false, removable: true, sessions };
}

const INPUT = {
    workspaces: [
        workspaceOf('workspace-a', [
            sessionOf('active-1', { updatedAt: 20 }),
            sessionOf('archived-1', { archived: true, updatedAt: 10 }),
            sessionOf('archived-2', { archived: true, updatedAt: 5 }),
        ]),
    ],
    fullPaths: {},
    query: '',
    orderBy: 'updated',
    selectedSessionId: null,
    activeSessionId: '',
    activeWorkspaceId: 'workspace-a',
};

function idsOf(archivedFilter) {
    return buildSidebarTree({ ...INPUT, archivedFilter })[0].sessions.map((s) => s.id);
}

test('隐藏已归档只留活跃会话', () => {
    assert.deepEqual(idsOf('hide'), ['active-1']);
});

test('全部对话同时给出活跃与归档会话', () => {
    assert.deepEqual(idsOf('show'), ['active-1', 'archived-1', 'archived-2']);
});

test('仅显示已归档只留归档会话', () => {
    // 用户报告的缺口：归档之后切到这一档，列表必须真的列出那几条归档会话。
    assert.deepEqual(idsOf('only'), ['archived-1', 'archived-2']);
});

test('归档会话仍按最近活动倒序，不因归档被打到末尾', () => {
    const archived = idsOf('only');
    assert.deepEqual(archived, ['archived-1', 'archived-2']);
});

test('搜索词同时作用于归档会话', () => {
    const filtered = buildSidebarTree({
        ...INPUT,
        archivedFilter: 'only',
        query: 'archived-2',
    });
    assert.deepEqual(
        filtered[0].sessions.map((s) => s.id),
        ['archived-2'],
    );
});
