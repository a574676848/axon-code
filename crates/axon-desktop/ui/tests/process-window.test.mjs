import test from 'node:test';
import assert from 'node:assert/strict';
import {
    LATEST_PROCESS_ROW_LIMIT,
    hiddenProcessRows,
} from '../src/components/conversation/turnWindow.ts';

/*
 * 最新一轮的过程行窗口。
 *
 * 这一组锁定三件事：只数过程行（叙述不参与）、保留的是最近的行、被收起的下标按
 * 出现顺序给出（第一根就是那句话的落点）。
 */

/** 造一行；`process` 与前端的判据同义。 */
function row(process) {
    return { process };
}

test('过程行不超过上限时一行都不收', () => {
    const rows = [row(true), row(false), row(true)];
    assert.deepEqual(hiddenProcessRows(rows, 10), []);
    const exact = Array.from({ length: LATEST_PROCESS_ROW_LIMIT }, () => row(true));
    assert.deepEqual(hiddenProcessRows(exact), [], '正好等于上限时也不收');
});

test('超出上限时只留最近的那几行，收起的是更早的', () => {
    const rows = Array.from({ length: 14 }, () => row(true));
    const hidden = hiddenProcessRows(rows, 10);
    assert.deepEqual(hidden, [0, 1, 2, 3], '14 行收 4 行，留下的是最后 10 行');
    assert.equal(hidden.length, rows.length - 10);
});

test('叙述不参与窗口：收起过程不会带走任何一句话', () => {
    const rows = [
        row(true),
        row(false),
        row(true),
        row(false),
        row(true),
        row(false),
        row(true),
    ];
    const hidden = hiddenProcessRows(rows, 2);
    assert.deepEqual(hidden, [0, 2], '只收过程行，叙述行的下标绝不出现在结果里');
    for (const index of hidden) assert.equal(rows[index].process, true);
});

test('收起提示落在第一根被收起的过程行上（下标升序）', () => {
    const rows = [row(false), row(true), row(true), row(true)];
    const hidden = hiddenProcessRows(rows, 1);
    assert.deepEqual(hidden, [1, 2]);
    assert.equal(hidden[0], 1, '第一根被收起的过程行就是替身行的位置');
});

test('上限为零时全部过程行收起，叙述照旧', () => {
    const rows = [row(true), row(false), row(true)];
    assert.deepEqual(hiddenProcessRows(rows, 0), [0, 2]);
});
