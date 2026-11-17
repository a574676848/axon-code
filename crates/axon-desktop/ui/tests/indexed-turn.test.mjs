import test from 'node:test';
import assert from 'node:assert/strict';
import { emptyTurn } from '../src/conversation/transcript/turns.ts';
import { indexedTurn, shouldFold } from '../src/conversation/transcript/fold.ts';
import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import { turnWindow } from '../src/components/conversation/turnWindow.ts';

/*
 * 索引轮：打开长会话时，更早的轮次按索引直接立起来，一条都不回放。
 *
 * 索引已经带着用户那一句、动作摘要与本轮最终交付正文，所以它立起来就是可读的一轮，
 * 不再是「点开才知道是什么」的占位行。这里固定形态、退化路径与渲染预算。
 */

/** 一条索引条目；默认是正常收口、带工具的一条。 */
function entry(overrides = {}) {
    return {
        interactionId: 'interaction-a',
        content: '把这一轮的历史读出来',
        receivedAtMs: 1_000,
        status: 'understood',
        toolSummary: 'read_file,shell_command',
        answerText: '读完了，结论在这里。',
        elapsedMs: 5_690,
        ...overrides,
    };
}

test('索引轮形态：用户气泡、过程控件、最终答案', () => {
    const turn = indexedTurn(entry());

    assert.equal(turn.id, 'interaction-a');
    assert.equal(turn.interactionId, 'interaction-a');
    assert.equal(turn.folded, true, '它已经处在折叠态');
    assert.equal(turn.keepExpanded, false, '索引轮不该占住「用户要它保持展开」');
    assert.deepEqual(
        turn.cells.map((cell) => cell.role),
        ['user', 'folded', 'assistant'],
    );

    const [user, control, answer] = turn.cells;
    assert.equal(user.text, '把这一轮的历史读出来');
    assert.equal(user.atMs, 1_000, '用户气泡的时间就是入站时间');
    assert.equal(control.summary, '已读取文件并执行了命令');
    assert.equal(control.elapsedMs, 5_690, '控件行尾的用时来自索引');
    assert.equal(answer.text, '读完了，结论在这里。');
    assert.equal(answer.atMs, null, '索引不含回答时间：不拿提问时间冒充');
});

test('纯问答轮不立过程控件：没有思考也没有工具就没有可折的过程', () => {
    const turn = indexedTurn(entry({ toolSummary: '', elapsedMs: 1_200 }));
    assert.deepEqual(
        turn.cells.map((cell) => cell.role),
        ['user', 'assistant'],
        '没有过程事实的一轮只留气泡与回答',
    );
    assert.equal(turn.folded, true, '轮次本身仍是收口态');
    assert.equal(turn.summary, '', '没有过程就不写摘要');
});

test('非正常收口：没有工具名同样不立控件，也不补空回答', () => {
    const interrupted = indexedTurn(
        entry({ status: 'interrupted', toolSummary: '', answerText: null }),
    );
    assert.equal(interrupted.status, 'interrupted');
    assert.deepEqual(
        interrupted.cells.map((cell) => cell.role),
        ['user'],
        '中断轮没有正文也没有过程时只剩用户气泡',
    );
    assert.deepEqual(
        indexedTurn(entry({ status: 'rejected', toolSummary: '' })).cells.map(
            (cell) => cell.role,
        ),
        ['user'],
        '驳回轮不补回答，也不立过程控件',
    );
});

test('索引轮不会再被自动折叠', () => {
    assert.equal(shouldFold(indexedTurn(entry()), 0, 2), false);
});

test('气泡正文与回放路径同源：结构化输入块与问答回执块都不进气泡', () => {
    const turn = indexedTurn(
        entry({
            content: '把这一轮读出来\n\n[结构化输入]\n已附加文件：x.png',
        }),
    );
    assert.equal(turn.cells[0].text, '把这一轮读出来');
});

test('前置一页不会重复接入同一条 Interaction', () => {
    const first = conversationReducer(INITIAL_CONVERSATION, {
        type: 'prepend-turns',
        turns: [indexedTurn(entry({ interactionId: 'a' })), indexedTurn(entry({ interactionId: 'b' }))],
    });
    assert.deepEqual(first.turns.map((turn) => turn.id), ['a', 'b']);
    const again = conversationReducer(first, {
        type: 'prepend-turns',
        turns: [indexedTurn(entry({ interactionId: 'b' })), indexedTurn(entry({ interactionId: 'c' }))],
    });
    assert.deepEqual(again.turns.map((turn) => turn.id), ['c', 'a', 'b'], '重叠的 b 不再接一遍');
});

test('渲染窗口不给折叠轮吃单元预算：老轮次不会被最新一轮挤掉', () => {
    const indexed = indexedTurn(entry({ interactionId: 'old' }));
    const newest = {
        ...emptyTurn('newest', 0),
        status: 'completed',
        cells: Array.from({ length: 300 }, (_, index) => ({
            id: `n:${index}`,
            role: 'assistant',
            text: '正文',
            streaming: false,
            atMs: null,
        })),
    };
    const window = turnWindow([indexed, newest], 240);
    assert.deepEqual(
        window.turns.map((item) => item.turn.id),
        ['old', 'newest'],
        '折叠轮始终在窗口里',
    );
    assert.equal(window.turns[0].hiddenCells, 0);
    assert.equal(window.turns[1].hiddenCells, 60, '预算只落在未折叠的那一轮上');
});

test('折叠轮有上限：翻得足够多时最老的整轮折回去', () => {
    const turns = Array.from({ length: 130 }, (_, index) =>
        indexedTurn(entry({ interactionId: `s${index}` })),
    );
    const window = turnWindow(turns, 240);
    assert.equal(window.turns.length, 120, '折叠轮上限生效');
    assert.equal(window.hiddenCells, 30, '超出的三轮各三格填进折叠提示');
});
