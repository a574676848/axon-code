import test from 'node:test';
import assert from 'node:assert/strict';
import { emptyTurn } from '../src/conversation/transcript/turns.ts';
import { processSummary, shouldFold } from '../src/conversation/transcript/fold.ts';
import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';

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

test('Axon 内置工具都落在既有类别里，不会掉进兜底', () => {
    // 这些名字取自 Axon 的 builtin 工具注册表：词表漏一个，摘要就会退化成「已调用工具」。
    const cases = [
        ['builtin.read_file', 'read'],
        ['builtin.read_dir', 'read'],
        ['builtin.search', 'search'],
        ['builtin.glob', 'search'],
        ['builtin.write_file', 'write'],
        ['builtin.edit', 'edit'],
        ['builtin.apply_patch', 'edit'],
        ['builtin.hashline_edit', 'edit'],
        ['builtin.shell_command', 'commands'],
        ['builtin.write_stdin', 'commands'],
        ['builtin.spawn_agent', 'subagents'],
        ['builtin.delegate_to_fast', 'subagents'],
        ['builtin.send_message', 'subagents'],
        ['builtin.goal_create', 'goalCreate'],
        ['builtin.goal_update', 'goal'],
        ['builtin.goal_start', 'goal'],
        ['builtin.goal_cancel', 'goal'],
        ['builtin.update_plan', 'plan'],
        ['builtin.ask_user_question', 'questions'],
        ['builtin.memory_search', 'memory'],
        ['builtin.invoke_skill', 'skill'],
    ];
    for (const [tool, category] of cases) {
        const summary = processSummary([cell(`c-${tool}`, 'tool', { toolName: tool })]);
        assert.notEqual(summary, '已调用工具', `${tool} 不该落进兜底类别`);
        assert.ok(summary.length > 0, `${tool} 必须有摘要`);
        assert.ok(category.length > 0);
    }
});
test('创建目标与更新计划是两件事，摘要不混用', () => {
    assert.equal(
        processSummary([cell('g', 'tool', { toolName: 'builtin.goal_create' })]),
        '创建了目标',
        '创建目标不能写成「更新了计划」',
    );
    assert.equal(
        processSummary([cell('p', 'plan')]),
        '更新了计划',
        '真正的计划更新仍写「更新了计划」',
    );
});

test('回放替换单元：有过程折成一行，没有过程就不插空抬头', () => {
    const plain = emptyTurn('interaction-plain', 1_000);
    const plainCells = [
        { id: 'u', role: 'user', text: '只回复 ok', atMs: 1_000, attachments: [] },
        { id: 'a', role: 'assistant', text: 'ok', streaming: false, atMs: 1_100 },
    ];
    const plainFolded = conversationReducer(
        { ...INITIAL_CONVERSATION, turns: [plain] },
        { type: 'replace-turn', turn: { ...plain, cells: plainCells }, mode: 'folded' },
    ).turns[0];
    assert.equal(
        plainFolded.cells.some((cell) => cell.role === 'folded'),
        false,
        '纯问答轮不立控件',
    );

    const worked = emptyTurn('interaction-worked', 2_000);
    const workedCells = [
        { id: 'u2', role: 'user', text: '列目录', atMs: 2_000, attachments: [] },
        { id: 'r2', role: 'reasoning', text: '想一下', done: true, durationMs: 5, startedAtMs: 2_000 },
        { id: 't2', role: 'tool', toolName: 'shell_command', toolKind: 'shell', command: 'ls', arguments: '', intent: '', status: 'completed', summary: '', output: 'a', durationMs: 3, startedAtMs: 2_000, exitCode: 0, jobId: null, jobStatus: null, fullOutputAvailable: false, filesystemWritePerformed: false, editPath: null, added: 0, removed: 0, diff: null },
        { id: 'a2', role: 'assistant', text: 'ok', streaming: false, atMs: 2_100 },
    ];
    const workedFolded = conversationReducer(
        { ...INITIAL_CONVERSATION, turns: [worked] },
        { type: 'replace-turn', turn: { ...worked, cells: workedCells }, mode: 'folded' },
    ).turns[0];
    assert.equal(workedFolded.cells.filter((cell) => cell.role === 'folded').length, 1);
    assert.equal(workedFolded.cells.some((cell) => cell.role === 'reasoning' || cell.role === 'tool'), false);
    assert.equal(workedFolded.folded, true);
});
