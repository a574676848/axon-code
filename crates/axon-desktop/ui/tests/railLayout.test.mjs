import test from 'node:test';
import assert from 'node:assert/strict';

import { DEFAULT_PAGE, EMPTY_LAYOUT, close, open, split } from '../src/sidebarRight/model.ts';

/*
 * 右侧停靠栏的开合口径：默认页不是内容。
 *
 * 内容落进只有默认页的那一格时默认页让位；整列只剩默认页时关掉内容即收起整列。
 * 两条合起来保证「右侧栏只开着一个文件，关掉它就收起」，同时不误伤一列里还有
 * 别的内容的情形。
 */

const FILE = {
    kind: 'preview',
    address: 'axon-resource://file/src%2Fmain.ts',
    title: 'main.ts',
};

const OTHER_FILE = {
    kind: 'preview',
    address: 'axon-resource://file/src%2Fapp.ts',
    title: 'app.ts',
};

const GUIDE = { kind: DEFAULT_PAGE.kind, address: null, title: DEFAULT_PAGE.title };

function kinds(layout) {
    return layout.panes.flatMap((pane) => pane.tabs.map((tab) => tab.kind));
}

test('关掉整列唯一的文件即收起整列，并退出全屏', () => {
    const opened = open(EMPTY_LAYOUT, FILE);
    assert.deepEqual(kinds(opened), ['preview']);
    assert.equal(opened.expanded, true);

    const closed = close({ ...opened, fullscreen: true }, opened.panes[0].tabs[0].id);
    assert.deepEqual(closed.panes, []);
    assert.equal(closed.expanded, false);
    assert.equal(closed.fullscreen, false);
});

test('默认页让位给从正文里打开的文件，不并排成两个 tab', () => {
    const withGuide = open(EMPTY_LAYOUT, GUIDE);
    assert.deepEqual(kinds(withGuide), ['guide']);

    const withFile = open(withGuide, FILE);
    assert.deepEqual(kinds(withFile), ['preview'], '落进来的文件顶替默认页');
    assert.equal(
        close(withFile, withFile.panes[0].tabs[0].id).expanded,
        false,
        '只剩一个文件时关掉它直接收起',
    );
});

test('整列只剩默认页时关掉最后一个内容 tab 也收起，默认页留在这一列里', () => {
    const withFile = open(EMPTY_LAYOUT, FILE);
    const withGuide = open(withFile, GUIDE);
    assert.deepEqual(kinds(withGuide), ['preview', 'guide']);

    const closed = close(withGuide, withGuide.panes[0].tabs[0].id);
    assert.equal(closed.expanded, false, '只画着一块引导页的空壳不留在展开态');
    assert.deepEqual(kinds(closed), ['guide'], '默认页等下次展开时原样回来');
});

test('整列还有别的内容时关掉其中一个文件不收起', () => {
    const first = open(EMPTY_LAYOUT, FILE);
    const both = open(first, OTHER_FILE);
    assert.deepEqual(kinds(both), ['preview', 'preview']);

    const closed = close(both, both.panes[0].tabs[1].id);
    assert.equal(closed.expanded, true);
    assert.deepEqual(kinds(closed), ['preview']);
    assert.equal(closed.panes[0].activeId, closed.panes[0].tabs[0].id, '落点回到左边那一个');
});

test('分栏新增的那一格只有默认页，打开的文件落到它这里', () => {
    const layout = split(open(EMPTY_LAYOUT, GUIDE));
    assert.equal(layout.panes.length, 2);

    const opened = open(layout, FILE);
    assert.deepEqual(kinds(opened), ['guide', 'preview'], '新内容顶替活跃格的默认页');
    assert.equal(opened.panes[1].activeId, opened.panes[1].tabs[0].id);
});
