import test from 'node:test';
import assert from 'node:assert/strict';
import { emptyTurn } from '../src/conversation/transcript/turns.ts';
import {
    historySkeletonTurn,
    shouldEvictFromWindow,
    shouldFold,
} from '../src/conversation/transcript/fold.ts';
import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import { turnWindow } from '../src/components/conversation/turnWindow.ts';

/*
 * 历史骨架轮：打开长会话时先按索引把更早的轮次立成折叠行，正文留着点开再取。
 * 这里固定三件事：形态与折叠轮一致（点开走同一条回放路径）、前置不重复、
 * 渲染窗口不能把它折没（折了就连点开的地方都没有）。
 */

/** 一条索引条目；默认是正常收口的一条。 */
function entry(overrides = {}) {
    return {
        interactionId: 'interaction-a',
        content: '把这一轮的历史读出来\n[结构化输入]\n已附加文件：x.png',
        receivedAtMs: 1_000,
        status: 'understood',
        ...overrides,
    };
}

test('骨架轮与折叠轮同形：只有一个折叠单元，标识取自 Interaction', () => {
    const turn = historySkeletonTurn(entry());
    assert.equal(turn.id, 'interaction-a');
    assert.equal(turn.interactionId, 'interaction-a');
    assert.equal(turn.folded, true);
    assert.equal(turn.keepExpanded, false, '骨架轮不该占住「用户要它保持展开」');
    assert.equal(turn.cells.length, 1);
    const [cell] = turn.cells;
    assert.equal(cell.role, 'folded');
    assert.equal(cell.interactionId, 'interaction-a');
    assert.equal(cell.prompt, '把这一轮的历史读出来', '只取用户敲的那一行');
    assert.equal(cell.atMs, 1_000);
});

test('状态与摘要按索引里的处理状态映射', () => {
    assert.equal(historySkeletonTurn(entry({ status: 'understood' })).status, 'completed');
    assert.equal(historySkeletonTurn(entry({ status: 'interrupted' })).status, 'interrupted');
    assert.equal(historySkeletonTurn(entry({ status: 'rejected' })).status, 'rejected');
    assert.match(historySkeletonTurn(entry({ status: 'interrupted' })).summary, /已中断/);
    assert.match(historySkeletonTurn(entry({ status: 'understood' })).summary, /已完成/);
});

test('前置一页骨架不会重复接入同一条 Interaction', () => {
    const first = conversationReducer(INITIAL_CONVERSATION, {
        type: 'prepend-turns',
        turns: [
            historySkeletonTurn(entry({ interactionId: 'a' })),
            historySkeletonTurn(entry({ interactionId: 'b' })),
        ],
    });
    assert.deepEqual(first.turns.map((turn) => turn.id), ['a', 'b']);
    const again = conversationReducer(first, {
        type: 'prepend-turns',
        turns: [
            historySkeletonTurn(entry({ interactionId: 'b' })),
            historySkeletonTurn(entry({ interactionId: 'c' })),
        ],
    });
    assert.deepEqual(again.turns.map((turn) => turn.id), ['c', 'a', 'b'], '重叠的 b 不再接一遍');
});

test('骨架轮不会再被自动折叠，也不按轮次窗口淘汰', () => {
    const skeleton = historySkeletonTurn(entry());
    assert.equal(shouldFold(skeleton), false);
    assert.equal(shouldEvictFromWindow(skeleton, 0, 40), false);
});

test('渲染窗口不给折叠轮吃预算：骨架行不会被最新一轮挤掉', () => {
    const skeleton = historySkeletonTurn(entry({ interactionId: 'old' }));
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
    const window = turnWindow([skeleton, newest], 240);
    assert.deepEqual(
        window.turns.map((item) => item.turn.id),
        ['old', 'newest'],
        '骨架行始终在窗口里',
    );
    assert.equal(window.turns[0].hiddenCells, 0);
    assert.equal(window.turns[1].hiddenCells, 60, '预算只落在正文那一轮上');
    assert.equal(window.hiddenCells, 60);
});

test('摘要在有模型时带上模型名，没有时留可操作的提示', () => {
    assert.match(
        historySkeletonTurn(entry({ model: 'deepseek-v4.1-flash' })).summary,
        /已完成 · deepseek-v4\.1-flash/,
    );
    assert.match(historySkeletonTurn(entry({ model: null })).summary, /点开加载/);
    assert.match(historySkeletonTurn(entry({ model: '   ' })).summary, /点开加载/, '空白模型名不占位');
});

test('索引带回工具串时，摘要就是那串动作短语', () => {
    const summary = historySkeletonTurn(
        entry({ toolSummary: 'read_file,shell_command,search,read_file' }),
    ).summary;
    assert.equal(summary, '已读取文件，执行了命令，已搜索代码', '同类去重、按出现先后排序');
});

test('工具串为空时退回状态与模型，不显示空摘要', () => {
    assert.match(
        historySkeletonTurn(entry({ toolSummary: '', model: 'deepseek-v4.1-flash' })).summary,
        /已完成 · deepseek-v4\.1-flash/,
    );
    assert.match(historySkeletonTurn(entry({ toolSummary: '   ' })).summary, /点开加载/);
});

test('折叠轮有上限：翻得足够多时最老的骨架折回去', () => {
    const skeletons = Array.from({ length: 130 }, (_, index) =>
        historySkeletonTurn(entry({ interactionId: `s${index}` })),
    );
    const window = turnWindow(skeletons, 240);
    assert.equal(window.turns.length, 120, '折叠轮上限生效');
    assert.equal(window.hiddenCells, 10, '超出的骨架按单元数计进折叠提示');
});
