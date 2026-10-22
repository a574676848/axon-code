import test from 'node:test';
import assert from 'node:assert/strict';
import { editingMenu, READONLY_COPY_ITEM } from '../src/components/ui/editingCommands.ts';

const noop = () => undefined;
const actions = {
    undo: noop,
    redo: noop,
    cut: noop,
    copy: noop,
    paste: noop,
    selectAll: noop,
};

test('可编辑菜单的条目、措辞与分组只有这一份定义', () => {
    const items = editingMenu(actions);
    assert.deepEqual(
        items.map(({ label }) => label),
        ['撤销', '恢复', '剪切', '复制', '粘贴', '全选'],
    );
    assert.deepEqual(
        items.map(({ shortcut }) => shortcut),
        ['Ctrl+Z', 'Ctrl+Y', 'Ctrl+X', 'Ctrl+C', 'Ctrl+V', 'Ctrl+A'],
    );
    // 分组线只落在剪切与全选之前：撤销恢复一组，剪切复制粘贴一组，全选收尾。
    assert.deepEqual(
        items.map(({ separatorBefore }) => separatorBefore === true),
        [false, false, true, false, false, true],
    );
});

test('未声明禁用状态的命令一律可用，声明了的按声明置灰', () => {
    const items = editingMenu(actions, { undo: true, copy: true });
    const disabled = Object.fromEntries(items.map(({ label, disabled: off }) => [label, off === true]));
    assert.deepEqual(disabled, {
        撤销: true,
        恢复: false,
        剪切: false,
        复制: true,
        粘贴: false,
        全选: false,
    });
});

test('只读菜单的文案与可编辑菜单里的复制项逐字一致', () => {
    const copy = editingMenu(actions).find(({ label }) => label === READONLY_COPY_ITEM.label);
    assert.equal(copy?.shortcut, READONLY_COPY_ITEM.shortcut);
});

test('每条命令都绑定到调用方给出的动作者', () => {
    const called = [];
    const spy = (name) => () => called.push(name);
    const items = editingMenu({
        undo: spy('undo'),
        redo: spy('redo'),
        cut: spy('cut'),
        copy: spy('copy'),
        paste: spy('paste'),
        selectAll: spy('selectAll'),
    });
    for (const item of items) item.action();
    assert.deepEqual(called, ['undo', 'redo', 'cut', 'copy', 'paste', 'selectAll']);
});
