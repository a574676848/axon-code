import test from 'node:test';
import assert from 'node:assert/strict';
import {
  caretLineBoundaryFallback,
  caretPlacement,
  CARET_ROW_TOLERANCE_PX,
} from '../src/components/conversation/composerCaretLine.ts';

test('亚像素取整不让同一行被当成两行', () => {
    // 同一行的行首上边界会因亚像素取整差不到 1px：那是同一行，不是换行。
    const same = caretPlacement(100 + 0.4, 100, 180);
    assert.deepEqual(same, { first: true, last: false });

    const tail = caretPlacement(180 - 0.4, 100, 180);
    assert.deepEqual(tail, { first: false, last: true });

    // 差一整行的行高就不是同一行了。
    const middle = caretPlacement(140, 100, 180);
    assert.deepEqual(middle, { first: false, last: false });
    assert.ok(
        Math.abs(140 - 100) > CARET_ROW_TOLERANCE_PX && Math.abs(140 - 180) > CARET_ROW_TOLERANCE_PX,
        '容差必须小于一行行高',
    );
});

test('只有一行时上键与下键都算在这一行上', () => {
    assert.deepEqual(caretPlacement(100, 100, 100), { first: true, last: true });
});

test('实测不可用时退回换行符：多行草稿的中间行两个方向都不翻历史', () => {
    const draft = '第一行\n第二行\n第三行';
    const middle = draft.indexOf('第二行');
    assert.deepEqual(caretLineBoundaryFallback(draft, middle), { first: false, last: false });

    assert.deepEqual(caretLineBoundaryFallback(draft, 0), { first: true, last: false });
    assert.deepEqual(caretLineBoundaryFallback(draft, draft.length), { first: false, last: true });
    // 空草稿：上键翻历史，下键没有可翻的（由历史导航自己返回 null）。
    assert.deepEqual(caretLineBoundaryFallback('', 0), { first: true, last: true });
});
