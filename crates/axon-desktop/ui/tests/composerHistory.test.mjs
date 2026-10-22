import test from 'node:test';
import assert from 'node:assert/strict';
import {
    COMPOSER_HISTORY_LIMIT,
    EMPTY_COMPOSER_HISTORY,
    canRedo,
    canUndo,
    recordEdit,
    redo,
    undo,
} from '../src/conversation/composerHistory.ts';

/*
 * 撤销与恢复的粒度契约。
 *
 * 这里按输入坞真实的接线方式驱动：每一次「变更事件」先记下编辑前的正文，再写入新正文。
 * 因此下面的用例同时钉住了两件事：模块本身的行为，以及组件必须怎样使用它。
 */

/** 一次变更事件：记下编辑前正文，再写入新正文。 */
function apply(state, before, after) {
    return { history: recordEdit(state.history, before), text: after };
}

/** 连续敲入若干字符，每个字符各算一次变更事件。 */
function typeChars(text, chars) {
    let current = { history: EMPTY_COMPOSER_HISTORY, text };
    for (const char of chars) {
        const next = current.text + char;
        current = apply(current, current.text, next);
    }
    return current;
}

test('逐字键入时按单个字符撤销与恢复', () => {
    let current = typeChars('', 'abc');
    assert.equal(current.text, 'abc');

    const first = undo(current.history, current.text);
    assert.equal(first.text, 'ab', '第一次撤销只回退一个字符');

    const second = undo(first.history, first.text);
    assert.equal(second.text, 'a', '第二次撤销再回退一个字符');

    const third = undo(second.history, second.text);
    assert.equal(third.text, '', '第三次撤销回到空草稿');

    assert.equal(undo(third.history, third.text), null, '没有可撤销的编辑时返回 null');

    const backOne = redo(third.history, third.text);
    assert.equal(backOne.text, 'a', '恢复同样一次一个字符');
    const backTwo = redo(backOne.history, backOne.text);
    assert.equal(backTwo.text, 'ab');
});

test('一次粘贴整段文本时按整段撤销与恢复', () => {
    // 粘贴在浏览器里只派发一次变更事件，因此只入栈一次。
    const pasted = apply({ history: EMPTY_COMPOSER_HISTORY, text: '开头' }, '开头', '开头粘贴进来的一大段');
    const undone = undo(pasted.history, pasted.text);
    assert.equal(undone.text, '开头', '一次撤销整段退回');
    assert.equal(undo(undone.history, undone.text), null, '整段粘贴只占一层历史');

    const redone = redo(undone.history, undone.text);
    assert.equal(redone.text, '开头粘贴进来的一大段', '一次恢复整段回来');
});

test('输入法组合按整个词组记一次，撤销时整词回退', () => {
    // 组合期间只更新画面不入栈，确认时才把组合前的正文记一次。
    const composed = apply({ history: EMPTY_COMPOSER_HISTORY, text: '' }, '', '中文词组');
    assert.equal(composed.history.past.length, 1, '整个词组只占一层历史');

    const undone = undo(composed.history, composed.text);
    assert.equal(undone.text, '', '撤销回到组合之前，而不是回退到拼音中间态');
});

test('发生新编辑后重做分支被丢弃', () => {
    const typed = typeChars('', 'ab');
    const undone = undo(typed.history, typed.text);
    assert.equal(canRedo(undone.history), true, '刚撤销完还有可恢复的分支');

    const retyped = apply(undone, undone.text, 'ax');
    assert.equal(canRedo(retyped.history), false, '新的编辑必须丢弃重做分支');
    assert.equal(canUndo(retyped.history), true);
});

test('撤销栈只保留最近若干次编辑，且仍按最新在前回退', () => {
    let state = { history: EMPTY_COMPOSER_HISTORY, text: '' };
    for (let index = 0; index < COMPOSER_HISTORY_LIMIT + 20; index += 1) {
        state = apply(state, state.text, `${state.text}x`);
    }
    assert.equal(state.history.past.length, COMPOSER_HISTORY_LIMIT, '历史深度受上限约束');

    const undone = undo(state.history, state.text);
    assert.equal(
        undone.text.length,
        state.text.length - 1,
        '超出上限后撤销的仍是最新一次编辑',
    );
});

test('空历史既不可撤销也不可恢复', () => {
    assert.equal(canUndo(EMPTY_COMPOSER_HISTORY), false);
    assert.equal(canRedo(EMPTY_COMPOSER_HISTORY), false);
    assert.equal(undo(EMPTY_COMPOSER_HISTORY, 'x'), null);
    assert.equal(redo(EMPTY_COMPOSER_HISTORY, 'x'), null);
});
