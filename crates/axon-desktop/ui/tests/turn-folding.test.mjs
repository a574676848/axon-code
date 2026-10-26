import test from 'node:test';
import assert from 'node:assert/strict';
import { emptyTurn } from '../src/conversation/transcript/turns.ts';
import {
    FOLD_CELL_THRESHOLD,
    FOLD_TEXT_THRESHOLD,
    MATERIALIZED_TURN_WINDOW,
    foldTurn,
    foldedSummary,
    isFoldCandidate,
    replaceTurnCells,
    shouldEvictFromWindow,
    shouldFold,
    turnTextWeight,
} from '../src/conversation/transcript/fold.ts';

/*
 * 折叠是本仓最要紧的一条内存约束：大轮次的正文必须在折叠那一刻真的被丢弃，
 * 而不是「藏起来」。因此这里既固定判据与摘要，也固定体量的释放效果。
 */

/** 造一轮：默认已收口。 */
function turnOf(cells, status = 'completed') {
    return { ...emptyTurn('interaction-1', 0), interactionId: 'interaction-1', status, cells };
}

function userCell(text) {
    return { id: 'u1', role: 'user', text, atMs: null, attachments: [] };
}

function toolCell(toolName, output = '') {
    return {
        id: `t:${toolName}`,
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

test('体量小的轮次不折，体量大的才折', () => {
    const small = turnOf([userCell('你好'), toolCell('read_file', '短输出')]);
    assert.equal(shouldFold(small), false, '小轮次折起来只会让人多按一次');

    const wide = turnOf([userCell('跑一遍'), toolCell('shell_command', 'x'.repeat(FOLD_TEXT_THRESHOLD))]);
    assert.equal(shouldFold(wide), true);
    assert.ok(turnTextWeight(wide) >= FOLD_TEXT_THRESHOLD);

    const many = turnOf(
        Array.from({ length: FOLD_CELL_THRESHOLD }, (_, index) => toolCell(`tool_${index}`, 'ok')),
    );
    assert.equal(shouldFold(many), true, '单元多的一轮同样要折');
});

test('运行中与用户已展开的轮次都不折', () => {
    const big = [userCell('跑一遍'), toolCell('shell_command', 'x'.repeat(FOLD_TEXT_THRESHOLD))];
    assert.equal(shouldFold(turnOf(big, 'streaming')), false, '正在长出来的一轮不能折');
    assert.equal(shouldFold({ ...turnOf(big), keepExpanded: true }), false, '点开过的不能被收回去');
    assert.equal(shouldFold({ ...turnOf(big), folded: true }), false, '已经折过的不重复折');
});

test('摘要按动作短语去重、最多三个，其余并成等', () => {
    const turn = turnOf([
        userCell('跑一遍'),
        toolCell('read_file'),
        toolCell('search'),
        toolCell('read_file'),
        toolCell('edit'),
        toolCell('shell_command'),
    ]);
    assert.equal(foldedSummary(turn), '已读取文件，已搜索代码，修改了文件等');
});

test('没有工具动作的长回复给出如实摘要，不留空行', () => {
    const plain = turnOf([
        userCell('说明一下'),
        { id: 'a1', role: 'assistant', text: 'x'.repeat(FOLD_TEXT_THRESHOLD), streaming: false, atMs: null },
    ]);
    assert.equal(foldedSummary(plain), '已生成长回复');
});

test('折叠真的释放正文：单元只剩一格，体量骤降', () => {
    const heavy = turnOf([
        userCell('跑一遍'),
        toolCell('shell_command', 'x'.repeat(80_000)),
        toolCell('read_file', 'y'.repeat(40_000)),
    ]);
    const before = turnTextWeight(heavy);
    assert.ok(before > 100_000);

    const folded = foldTurn(heavy, foldedSummary(heavy));
    assert.equal(folded.folded, true);
    assert.equal(folded.cells.length, 1, '折叠后只留一行摘要');
    assert.equal(folded.cells[0].role, 'folded');
    assert.equal(folded.cells[0].prompt, '跑一遍', '用户那一句要留在折叠行上');
    assert.ok(
        turnTextWeight(folded) < 100,
        `折叠后的体量必须回到百字符以内，实际 ${turnTextWeight(folded)}`,
    );
    // 旧单元已经不在任何字段里：序列化后的体量同样骤降。
    assert.ok(JSON.stringify(folded).length < 2_000, '被丢弃的正文不得残留引用');
});

test('展开换回正文并标记保持展开', () => {
    const heavy = turnOf([userCell('跑一遍'), toolCell('shell_command', 'x'.repeat(50_000))]);
    const folded = foldTurn(heavy, foldedSummary(heavy));
    const restored = replaceTurnCells(folded, heavy.cells);

    assert.equal(restored.folded, false);
    assert.equal(restored.keepExpanded, true, '展开过的一轮不再被自动折叠');
    assert.equal(restored.cells.length, heavy.cells.length);
    assert.equal(restored.index[heavy.cells[1].id], 1, '下标索引要跟着重建');
    assert.equal(shouldFold(restored), false);
});

test('发送新一轮不会折叠上一轮：最近两轮始终展开', () => {
    const heavyTurn = () =>
        turnOf([userCell('跑一遍'), toolCell('shell_command', 'x'.repeat(FOLD_TEXT_THRESHOLD))]);
    /** 照 useAutoFoldTurns 的口径跑一遍：先过位置闸门，再按体量判。 */
    const settle = (turns) =>
        turns.map((turn, index) =>
            isFoldCandidate(index, turns.length) && shouldFold(turn)
                ? foldTurn(turn, foldedSummary(turn))
                : turn,
        );

    const justSent = settle([heavyTurn(), turnOf([userCell('新一轮')])]);
    assert.equal(justSent[0].folded, false, '按下发送的同一刻，上一轮必须仍然展开');
    assert.equal(justSent[0].cells.length, 2, '上一轮的正文不能被摘要顶掉');

    const later = settle([
        heavyTurn(),
        turnOf([userCell('第二轮')]),
        turnOf([userCell('第三轮')]),
    ]);
    assert.equal(later[0].folded, true, '隔了两轮之后才按体量回收');
    assert.equal(later[1].folded, false, '上一轮仍然展开');
});

test('位置闸门只认最近两轮之外的老轮次', () => {
    assert.equal(isFoldCandidate(0, 1), false, '唯一的一轮永远不折');
    assert.equal(isFoldCandidate(0, 2), false, '两条轮次时，上一轮不会被收');
    assert.equal(isFoldCandidate(1, 2), false, '正在跑的一轮同样不折');
    assert.equal(isFoldCandidate(0, 3), true, '再往前的一轮才轮到体量判据');
    assert.equal(isFoldCandidate(1, 3), false);
    assert.equal(isFoldCandidate(2, 3), false);
});

test('轮次窗口：超出保留窗口的老轮次一律淘汰，最新一轮永不淘汰', () => {
    const total = MATERIALIZED_TURN_WINDOW + 3;
    const turns = Array.from({ length: total }, (_, index) => turnOf([userCell(`第 ${index} 轮`)]));

    assert.equal(shouldEvictFromWindow(turns[0], 0, total), true, '最老的一轮必须回收');
    assert.equal(shouldEvictFromWindow(turns[2], 2, total), true, '窗口之外同样回收');
    assert.equal(shouldEvictFromWindow(turns[3], 3, total), false, '窗口之内保持展开');
    assert.equal(
        shouldEvictFromWindow(turns[total - 1], total - 1, total),
        false,
        '正在看的一轮不能被回收',
    );
});

test('窗口淘汰只动可回收的轮次：运行中与用户展开过的都不动', () => {
    const total = MATERIALIZED_TURN_WINDOW + 2;
    const live = turnOf([userCell('正在跑')], 'streaming');
    assert.equal(shouldEvictFromWindow(live, 0, total), false, '运行中的一轮不能回收');

    const opened = { ...turnOf([userCell('刚看过')]), keepExpanded: true };
    assert.equal(shouldEvictFromWindow(opened, 0, total), false, '刚点开的一轮不该被回收');

    const already = { ...turnOf([userCell('已折')]), folded: true };
    assert.equal(shouldEvictFromWindow(already, 0, total), false, '折过的不重复折');
});

test('多轮会话按窗口回收后，被淘汰轮次的正文体量回到摘要级', () => {
    // 每轮都带一段中等输出：单看体量不足以触发折叠，但轮数一多就会堆起来。
    const perTurn = 1_000;
    const total = MATERIALIZED_TURN_WINDOW + 4;
    let turns = Array.from({ length: total }, (_, index) =>
        turnOf([userCell(`第 ${index} 轮`), toolCell('read_file', 'z'.repeat(perTurn))]),
    );
    const before = turns.reduce((sum, turn) => sum + turnTextWeight(turn), 0);
    assert.ok(before > total * perTurn, `回收前应包含每轮的正文，实际 ${before}`);

    turns = turns.map((turn, index) =>
        shouldEvictFromWindow(turn, index, total) ? foldTurn(turn, foldedSummary(turn)) : turn,
    );

    // 窗口保留最近 MATERIALIZED_TURN_WINDOW 轮，其余全部淘汰。
    const evicted = total - MATERIALIZED_TURN_WINDOW;
    const foldedTurns = turns.filter((turn) => turn.folded);
    assert.equal(foldedTurns.length, evicted);
    for (const turn of foldedTurns) {
        assert.ok(
            turnTextWeight(turn) < 200,
            `被淘汰的一轮只该剩下摘要，实际 ${turnTextWeight(turn)}`,
        );
    }
    const after = turns.reduce((sum, turn) => sum + turnTextWeight(turn), 0);
    assert.ok(
        before - after > evicted * perTurn * 0.9,
        `回收掉的体量应约等于 ${evicted} 轮正文，实际 ${before - after}`,
    );
});
