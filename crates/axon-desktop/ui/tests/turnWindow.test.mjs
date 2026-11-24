import test from 'node:test';
import assert from 'node:assert/strict';
import { turnWindow } from '../src/components/conversation/turnWindow.ts';
import { emptyTurn } from '../src/conversation/transcript/turns.ts';

/*
 * 正文窗口按轮次切：这是渲染边界的基础，因此这里把它的三条口径固定下来。
 */

/** 造一轮：按给定单元数铺出占位单元。 */
function turnOf(id, cellCount) {
    const turn = emptyTurn(id, 0);
    return {
        ...turn,
        status: 'completed',
        cells: Array.from({ length: cellCount }, (_, index) => ({
            id: `${id}:${index}`,
            role: 'assistant',
            text: `第 ${index} 格`,
            streaming: false,
            atMs: null,
        })),
    };
}

test('预算之内整轮保留，顺序仍由早到晚', () => {
    const turns = [turnOf('a', 3), turnOf('b', 4)];
    const window = turnWindow(turns, 10);
    assert.equal(window.hiddenCells, 0);
    assert.deepEqual(
        window.turns.map((entry) => entry.turn.id),
        ['a', 'b'],
    );
    assert.deepEqual(
        window.turns.map((entry) => entry.hiddenCells),
        [0, 0],
    );
});

test('预算不足时从最早一轮折起，最新一轮始终完整可见', () => {
    const turns = [turnOf('a', 5), turnOf('b', 5), turnOf('c', 5)];
    const window = turnWindow(turns, 7);
    assert.deepEqual(
        window.turns.map((entry) => entry.turn.id),
        ['b', 'c'],
        '最早的一轮整块折掉',
    );
    assert.equal(window.hiddenCells, 8, '折掉 a 的 5 格与 b 的前 3 格');
    assert.deepEqual(
        window.turns.map((entry) => entry.hiddenCells),
        [3, 0],
        '只有跨在边界上的那一轮折前导单元',
    );
});

test('大轮次不会被整块折掉：预算够就保留它的最新部分', () => {
    const turns = [turnOf('huge', 120)];
    const window = turnWindow(turns, 40);
    assert.equal(window.turns.length, 1);
    assert.equal(window.turns[0].hiddenCells, 80);
    assert.equal(
        window.turns[0].turn.cells.length - window.turns[0].hiddenCells,
        40,
        '窗口内应恰好保留预算数量的单元',
    );
});

test('预算为零时不保留任何单元', () => {
    const turns = [turnOf('a', 2), turnOf('b', 3)];
    const window = turnWindow(turns, 0);
    assert.equal(window.turns.length, 0);
    assert.equal(window.hiddenCells, 5);
});
