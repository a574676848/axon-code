import test from 'node:test';
import assert from 'node:assert/strict';
import {
  caretOnFirstLine,
  caretOnLastLine,
  historyNext,
  historyPrevious,
  IDLE_HISTORY,
} from '../src/conversation/inputHistory.ts';

const entries = ['最早一句', '中间一句', '最近一句'];

test('上键从最新一条开始往前翻，并记住开始导航前的草稿', () => {
    const first = historyPrevious(entries, IDLE_HISTORY, '我正在写的草稿');
    assert.deepEqual(first, {
        navigation: { index: 2, draft: '我正在写的草稿' },
        text: '最近一句',
    });

    // 继续往前：草稿锚点保持不变（不会被中途输入覆盖）。
    const second = historyPrevious(entries, first.navigation, '忽略');
    assert.deepEqual(second, { navigation: { index: 1, draft: '我正在写的草稿' }, text: '中间一句' });
    const third = historyPrevious(entries, second.navigation, '忽略');
    assert.deepEqual(third, { navigation: { index: 0, draft: '我正在写的草稿' }, text: '最早一句' });

    // 到最早一条就停住，不再往前。
    const stalled = historyPrevious(entries, third.navigation, '忽略');
    assert.deepEqual(stalled, { navigation: { index: 0, draft: '我正在写的草稿' }, text: '最早一句' });
});

test('下键走到最新一条之后归还草稿，而不是把历史留在输入框里', () => {
    // 先翻到更早的一条，再往下走回最新一条；再往下就回到当初的草稿。
    const newest = historyPrevious(entries, IDLE_HISTORY, '草稿');
    const older = historyPrevious(entries, newest.navigation, '忽略');
    assert.equal(older.text, '中间一句');

    const back = historyNext(entries, older.navigation);
    assert.deepEqual(back, { navigation: { index: 2, draft: '草稿' }, text: '最近一句' });

    const restored = historyNext(entries, back.navigation);
    assert.deepEqual(restored, { navigation: IDLE_HISTORY, text: '草稿' });

    // 没在导航时下键不做事。
    assert.equal(historyNext(entries, IDLE_HISTORY), null);
});

test('没有历史时上键不做事', () => {
    assert.equal(historyPrevious([], IDLE_HISTORY, '草稿'), null);
    // 下键在越界之后归还草稿：草稿缺失时给出空串，而不是 undefined。
    const restored = historyNext(entries, { index: 2, draft: null });
    assert.deepEqual(restored, { navigation: IDLE_HISTORY, text: '' });
});

test('只有首行上键、末行下键才翻历史', () => {
    const multiline = '第一行\n第二行';
    assert.equal(caretOnFirstLine(multiline, 0), true);
    assert.equal(caretOnFirstLine(multiline, 3), true);
    assert.equal(caretOnFirstLine(multiline, 5), false, '光标在第二行时上键要移动光标');

    assert.equal(caretOnLastLine(multiline, 3), false);
    assert.equal(caretOnLastLine(multiline, 5), true);
    assert.equal(caretOnLastLine(multiline, multiline.length), true);
});
