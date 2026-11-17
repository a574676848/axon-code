import test from 'node:test';
import assert from 'node:assert/strict';
import { emptyTurn } from '../src/conversation/transcript/turns.ts';
import {
    foldTurn,
    processSummary,
    replaceTurnCells,
    shouldFold,
} from '../src/conversation/transcript/fold.ts';

/*
 * 整轮折叠：过程整段撤下、控件插在开场输入之后、内容原位保留。
 *
 * 这里固定三件事，缺一件这次改造就退化成「把整轮压成一行」：
 * 判据（谁能折）、形态与顺序（折完还是原话顺序）、释放（过程正文真的不再被引用）。
 */

/** 造一轮：默认已收口。 */
function turnOf(cells, status = 'completed') {
    return { ...emptyTurn('interaction-1', 0), interactionId: 'interaction-1', status, cells };
}

function userCell(text, id = 'u1') {
    return { id, role: 'user', text, atMs: null, attachments: [] };
}

function assistantCell(text, id) {
    return { id, role: 'assistant', text, streaming: false, atMs: null };
}

function toolCell(toolName, output = '', id = `t:${toolName}`) {
    return {
        id,
        role: 'tool',
        toolName,
        toolKind: 'builtin',
        command: '',
        arguments: '',
        intent: '',
        status: 'completed',
        summary: '',
        output,
        durationMs: null,
        startedAtMs: null,
        exitCode: null,
        jobId: null,
        fullOutputAvailable: false,
        filesystemWritePerformed: false,
        editPath: null,
        added: 0,
        removed: 0,
        diff: null,
    };
}

test('只有最新的那一轮永远展开', () => {
    const heavy = turnOf([userCell('跑一遍'), toolCell('shell_command', 'x'.repeat(50_000))]);
    assert.equal(shouldFold(heavy, 0, 1), false, '唯一的一轮不折');
    assert.equal(shouldFold(heavy, 1, 2), false, '最新一轮不折');
    assert.equal(shouldFold(heavy, 0, 2), true, '再往前一轮就该折');
});

test('形态：用户气泡、过程控件、回复原位，一句话都不搬动', () => {
    const turn = turnOf([
        userCell('先看看现状'),
        assistantCell('我先读一下这两个文件。', 'a1'),
        toolCell('read_file', '正文 A'),
        assistantCell('读完了，再跑一次测试。', 'a2'),
        toolCell('shell_command', '正文 B'),
        assistantCell('结果在这里。', 'a3'),
    ]);
    const folded = foldTurn(turn);

    assert.equal(folded.folded, true);
    assert.deepEqual(
        folded.cells.map((cell) => cell.role),
        ['user', 'folded', 'assistant', 'assistant', 'assistant'],
        '控件紧跟开场输入，三段叙述保持原有先后',
    );
    assert.deepEqual(
        folded.cells.filter((cell) => cell.role === 'assistant').map((cell) => cell.text),
        ['我先读一下这两个文件。', '读完了，再跑一次测试。', '结果在这里。'],
        '叙述 A 与叙述 B 不得互换，也不得被挪到控件之前',
    );
    const control = folded.cells[1];
    assert.equal(control.role, 'folded');
    assert.equal(control.interactionId, 'interaction-1', '点开回放要认得出是哪一条');
    assert.equal(control.summary, '已读取文件并执行了命令');
    assert.equal(folded.index[control.id], 1, '下标索引要跟着重建');
});

test('过程整段撤下：工具输出不再被任何字段引用', () => {
    const heavy = turnOf([
        userCell('跑一遍'),
        toolCell('shell_command', 'x'.repeat(80_000)),
        toolCell('read_file', 'y'.repeat(40_000)),
    ]);
    const folded = foldTurn(heavy);

    assert.ok(JSON.stringify(folded).length < 2_000, '被释放的正文不得残留引用');
    assert.equal(
        folded.cells.some((cell) => cell.role === 'tool'),
        false,
        '折叠态不保留工具单元',
    );
});

test('没有过程的轮次不折：折了与没折完全一样', () => {
    const plain = turnOf([userCell('说明一下'), assistantCell('说明如下。', 'a1')]);
    assert.equal(shouldFold(plain, 0, 2), false);
    assert.equal(foldTurn(plain).folded, false, '没有可撤下的过程就不该插控件');
});

test('过程之后又收到输入的轮次不折：控件只能落在开场输入之后', () => {
    const steering = turnOf([
        userCell('跑一遍'),
        toolCell('shell_command'),
        userCell('等一下，先别跑', 'u2'),
    ]);
    assert.equal(shouldFold(steering, 0, 2), false, '把这条输入压到过程之前会读错顺序');

    const opening = turnOf([
        userCell('第一句'),
        userCell('补充一句', 'u1b'),
        toolCell('shell_command'),
    ]);
    const folded = foldTurn(opening);
    assert.deepEqual(
        folded.cells.map((cell) => cell.role),
        ['user', 'user', 'folded'],
        '开场连续输入都在控件之上',
    );
});

test('运行中、中断、失败、已折、用户展开过的一律不折', () => {
    const cells = [userCell('跑一遍'), toolCell('shell_command')];
    assert.equal(shouldFold(turnOf(cells, 'streaming'), 0, 2), false);
    assert.equal(shouldFold(turnOf(cells, 'pending'), 0, 2), false);
    assert.equal(shouldFold(turnOf(cells, 'interrupted'), 0, 2), false, '中断的结论要保持可见');
    assert.equal(shouldFold(turnOf(cells, 'failed'), 0, 2), false, '失败卡要保持可见');
    assert.equal(shouldFold({ ...turnOf(cells), folded: true }, 0, 2), false);
    assert.equal(shouldFold({ ...turnOf(cells), keepExpanded: true }, 0, 2), false);
});

test('摘要按动作短语去重、最多三个，其余并成等', () => {
    const turn = turnOf([
        userCell('跑一遍'),
        toolCell('read_file'),
        toolCell('search'),
        toolCell('read_file', '', 't:read_file:2'),
        toolCell('edit'),
        toolCell('shell_command'),
    ]);
    assert.equal(processSummary(turn.cells), '已读取文件，已搜索代码，修改了文件等', '按次数排名：读取两次排第一');
});

test('没有工具的过程按类别给一句，不写「已生成长回复」这类含糊话', () => {
    const reasoning = turnOf([
        userCell('想一想'),
        { id: 'r1', role: 'reasoning', text: '思考正文', done: true, durationMs: null, startedAtMs: null },
    ]);
    assert.equal(processSummary(reasoning.cells), '已完成分析', '只有思考时用分析文案');

    const changes = turnOf([
        userCell('改一下'),
        { id: 'f1', role: 'fileChange', path: 'a.ts', changeType: 'edit', added: 1, removed: 0, diff: '' },
    ]);
    assert.equal(processSummary(changes.cells), '修改了文件', '文件改动仍归到编辑类别');
});

test('展开换回正文并标记保持展开', () => {
    const heavy = turnOf([userCell('跑一遍'), toolCell('shell_command', 'x'.repeat(50_000))]);
    const folded = foldTurn(heavy);
    const restored = replaceTurnCells(folded, heavy.cells);

    assert.equal(restored.folded, false);
    assert.equal(restored.keepExpanded, true, '展开过的一轮不再被自动折叠');
    assert.equal(restored.expanded, true, '展开态由控件箭头表达');
    // 展开态仍保留过程控件（在开场输入之后），因此比原单元多一根抬头。
    assert.equal(restored.cells.length, heavy.cells.length + 1);
    assert.equal(restored.cells[1].role, 'folded');
    assert.equal(restored.cells[1].expanded, true);
    assert.equal(restored.index[heavy.cells[1].id], 2, '下标索引要跟着重建');
    assert.equal(shouldFold(restored, 0, 2), false);
});

test('多轮会话按位置逐轮折叠，最新一轮保持完整', () => {
    const make = (id) =>
        ({
            ...turnOf([userCell(`第 ${id} 轮`), toolCell('read_file', '输出', `t:${id}`)]),
            id,
            interactionId: id,
        });
    const turns = [make('a'), make('b'), make('c')];
    const settled = turns.map((turn, index) =>
        shouldFold(turn, index, turns.length) ? foldTurn(turn) : turn,
    );
    assert.deepEqual(
        settled.map((turn) => turn.folded),
        [true, true, false],
        '除最新一轮之外都折起来',
    );
    assert.equal(settled[2].cells.length, 2, '最新一轮的正文一格都不能少');
});
