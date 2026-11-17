import test from 'node:test';
import assert from 'node:assert/strict';
import { emptyTurn } from '../src/conversation/transcript/turns.ts';
import { indexedTurn, processSummary, shouldFold } from '../src/conversation/transcript/fold.ts';

/*
 * 折叠行的摘要是这一轮「做过什么」的唯一可见说明：读者不展开也要能读出思考与动作。
 * 这里固定三件事：思考进摘要、动作按首次出现顺序去重、没有过程就不立控件。
 */

function cell(id, role, extra = {}) {
    return { id, role, atMs: null, ...extra };
}

test('思考与工具并列进摘要，思考在前', () => {
    const summary = processSummary([
        cell('a', 'reasoning'),
        cell('b', 'tool', { toolName: 'shell_command' }),
        cell('c', 'tool', { toolName: 'read_file' }),
    ]);
    assert.equal(summary, '执行了命令并已读取文件', '两条类别用「并」连接，第二条省略共用的「已」');
});

test('同类动作只出现一次，超过三条并成「等」', () => {
    const summary = processSummary([
        cell('a', 'tool', { toolName: 'shell_command' }),
        cell('b', 'tool', { toolName: 'builtin.shell_command' }),
        cell('c', 'tool', { toolName: 'read_file' }),
        cell('d', 'tool', { toolName: 'edit' }),
        cell('e', 'tool', { toolName: 'search' }),
    ]);
    assert.equal(summary, '执行了命令，已读取文件，修改了文件等');
});

test('只有思考时摘要写已思考，而不是空串', () => {
    assert.equal(processSummary([cell('a', 'reasoning')]), '已完成分析');
});

test('没有思考也没有工具：不折、不留控件', () => {
    const turn = emptyTurn('interaction-quiet', 1_000);
    turn.cells = [
        { id: 'u', role: 'user', text: '你好', atMs: 1_000, attachments: [] },
        { id: 'a', role: 'assistant', text: '你好', streaming: false, atMs: null },
    ];
    assert.equal(shouldFold(turn, 0, 3), false, '纯问答轮不折');
    const indexed = indexedTurn({
        interactionId: 'interaction-quiet',
        content: '你好',
        receivedAtMs: 1_000,
        status: 'understood',
        toolSummary: '',
        answerText: '你好',
        elapsedMs: 800,
    });
    assert.equal(
        indexed.cells.some((entry) => entry.role === 'folded'),
        false,
        '索引轮没有工具名时也不立控件',
    );
});

test('有思考或工具的轮次照旧可折', () => {
    const turn = emptyTurn('interaction-busy', 1_000);
    turn.status = 'completed';
    turn.cells = [
        { id: 'u', role: 'user', text: '跑一下', atMs: 1_000, attachments: [] },
        cell('r', 'reasoning'),
        cell('t', 'tool', { toolName: 'shell_command' }),
        { id: 'a', role: 'assistant', text: '好了', streaming: false, atMs: null },
    ];
    assert.equal(shouldFold(turn, 0, 3), true);
});
