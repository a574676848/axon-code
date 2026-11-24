import test from 'node:test';
import assert from 'node:assert/strict';
import { draftEntriesFromJson, isPersonalKind, isWorkspaceKind } from '../src/components/memoryDraft.ts';

test('五值类型的归属作用域固定', () => {
    assert.equal(isPersonalKind('profile'), true);
    assert.equal(isPersonalKind('preference'), true);
    assert.equal(isWorkspaceKind('experience'), true);
    assert.equal(isWorkspaceKind('knowledge'), true);
    assert.equal(isWorkspaceKind('procedure'), true);
    assert.equal(isPersonalKind('knowledge'), false);
});

test('JSON 录入接受单条对象、数组与 records 包装体', () => {
    const single = draftEntriesFromJson('{"memoryKind":"preference","summary":" 偏好 ","evidence":" e "}');
    assert.deepEqual(single.errors, []);
    assert.deepEqual(single.entries, [
        { memoryKind: 'preference', summary: '偏好', evidence: 'e', workspacePath: undefined, supersedesMemoryIds: undefined },
    ]);

    const array = draftEntriesFromJson(
        '[{"memoryKind":"knowledge","summary":"s","evidence":"e","workspacePath":"E:/repo"},{"memoryKind":"procedure","summary":"s2","evidence":"e2","workspacePath":"E:/repo"}]',
    );
    assert.deepEqual(array.errors, []);
    assert.equal(array.entries.length, 2);
    assert.equal(array.entries[0].workspacePath, 'E:/repo');

    const wrapped = draftEntriesFromJson(
        '{"records":[{"memoryKind":"experience","summary":"s","evidence":"e","workspacePath":"E:/repo","supersedesMemoryIds":[9]}]}',
    );
    assert.deepEqual(wrapped.errors, []);
    assert.deepEqual(wrapped.entries[0].supersedesMemoryIds, ['9']);

    const stringIds = draftEntriesFromJson(
        '{"records":[{"memoryKind":"experience","summary":"s","evidence":"e","workspacePath":"E:/repo","supersedesMemoryIds":["8051494672233320981"]}]}',
    );
    assert.deepEqual(
        stringIds.entries[0].supersedesMemoryIds,
        ['8051494672233320981'],
        '超出 JS 安全整数区间的 id 必须以字符串原样透传',
    );
});

test('仓库记忆缺少文件夹目录必须被本地拦下', () => {
    const missing = draftEntriesFromJson('{"memoryKind":"knowledge","summary":"s","evidence":"e"}');
    assert.equal(missing.entries.length, 0);
    assert.match(missing.errors[0], /workspacePath/);

    const blank = draftEntriesFromJson('{"memoryKind":"procedure","summary":"s","evidence":"e","workspacePath":"   "}');
    assert.equal(blank.entries.length, 0);
    assert.match(blank.errors[0], /workspacePath/);
});

test('Personal 记忆不得携带文件夹目录', () => {
    const result = draftEntriesFromJson(
        '{"memoryKind":"preference","summary":"s","evidence":"e","workspacePath":"E:/repo"}',
    );
    assert.equal(result.entries.length, 0);
    assert.match(result.errors[0], /Personal/);
});

test('非法类型、空字段与未知字段逐条报错且不产生半批写入', () => {
    const illegalKind = draftEntriesFromJson('{"memoryKind":"hunch","summary":"s","evidence":"e"}');
    assert.match(illegalKind.errors[0], /memoryKind/);

    const emptySummary = draftEntriesFromJson('{"memoryKind":"preference","summary":"  ","evidence":"e"}');
    assert.match(emptySummary.errors[0], /summary/);

    const unknown = draftEntriesFromJson(
        '{"records":[{"memoryKind":"knowledge","summary":"s","evidence":"e","workspacePath":"E:/repo"},{"memoryKind":"knowledge","summary":"s","evidence":"e","workspacePath":"E:/repo","userRef":"spoofed"}]}',
    );
    assert.equal(unknown.entries.length, 0, '任一非法条目都必须让整批停止，避免半批写入');
    assert.match(unknown.errors[0], /不支持字段/);

    assert.match(draftEntriesFromJson('{"records":[]}').errors[0], /没有可写入/);
    assert.match(draftEntriesFromJson('not json').errors[0], /JSON 解析失败/);
    assert.match(draftEntriesFromJson('"text"').errors[0], /必须是对象/);
});
