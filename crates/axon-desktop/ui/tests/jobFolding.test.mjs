import test from 'node:test';
import assert from 'node:assert/strict';
import { foldJobReads, jobOutputJobId } from '../src/components/conversation/jobFolding.ts';

/*
 * 同一轮里反复读同一个作业是实测最常见的噪音：一次等待能连出十几条一模一样的行。
 * 折叠只并**相邻**的同类读取，因此这里固定四条边界：相邻才并、隔着别的动作不并、
 * 不同作业不并、并完能还原原貌（次数与合计耗时对得上）。
 */

function jobCell(id, jobId, durationMs = null) {
    return {
        id,
        role: 'tool',
        toolName: 'builtin.job_output',
        toolKind: 'job',
        command: '',
        arguments: JSON.stringify({ job_id: jobId }),
        intent: '',
        status: 'completed',
        summary: '',
        output: '',
        durationMs,
        startedAtMs: 0,
        exitCode: null,
        jobId: null,
        jobStatus: null,
        fullOutputAvailable: false,
        filesystemWritePerformed: false,
        editPath: null,
        added: 0,
        removed: 0,
        diff: null,
    };
}

/** 同为工具单元，但读的不是作业：它出现在中间就说明这是两次独立的观察。 */
function otherCell(id) {
    return { ...jobCell(id, '55'), toolName: 'builtin.shell_command', arguments: '{"command":"ls"}' };
}

test('相邻、同作业的读取并成一条，留下最新的一条', () => {
    const folded = foldJobReads([
        jobCell('a', '55', 1010),
        jobCell('b', '55', 1010),
        jobCell('c', '55', 104),
    ]);
    assert.equal(folded.length, 1);
    assert.equal(folded[0].cell.id, 'c');
    assert.equal(folded[0].repeats, 3);
    assert.equal(folded[0].repeatsMs, 2124);
});

test('中间隔着别的动作时不并', () => {
    const folded = foldJobReads([jobCell('a', '55'), otherCell('s'), jobCell('b', '55')]);
    assert.deepEqual(
        folded.map((item) => item.cell.id),
        ['a', 's', 'b'],
    );
});

test('不同作业相邻也不并', () => {
    const folded = foldJobReads([jobCell('a', '55'), jobCell('b', '56')]);
    assert.deepEqual(
        folded.map((item) => item.cell.id),
        ['a', 'b'],
    );
});

test('并过之后的顺序与次数仍能还原原来的单元序列', () => {
    const folded = foldJobReads([
        jobCell('a', '55'),
        jobCell('b', '55'),
        otherCell('s'),
        jobCell('c', '55'),
    ]);
    assert.deepEqual(
        folded.map((item) => [item.cell.id, item.repeats]),
        [
            ['b', 2],
            ['s', 1],
            ['c', 1],
        ],
    );
});

test('读的不是作业的工具不受影响', () => {
    const folded = foldJobReads([
        otherCell('s1'),
        otherCell('s2'),
    ]);
    assert.equal(folded.length, 2);
    assert.equal(folded[0].repeats, 1);
});

test('取不到耗时时合计为空，只有一侧缺失按 0 累加', () => {
    assert.equal(foldJobReads([jobCell('a', '55'), jobCell('b', '55')])[0].repeatsMs, null);
    assert.equal(foldJobReads([jobCell('a', '55'), jobCell('b', '55', 500)])[0].repeatsMs, 500);
});

test('单条读取保持原样', () => {
    const folded = foldJobReads([jobCell('a', '55', 1010)]);
    assert.equal(folded[0].repeats, 1);
    assert.equal(folded[0].repeatsMs, 1010);
});

test('作业标识从调用参数里取，认不下就当作不是作业读取', () => {
    assert.equal(jobOutputJobId('builtin.job_output', '{"job_id":"55"}'), '55');
    assert.equal(jobOutputJobId('builtin.job_output', ' {"jobId": 55} '), '55');
    assert.equal(jobOutputJobId('job_output', '{"job_id":"7"}'), '7');
    assert.equal(jobOutputJobId('builtin.job_output', '{"job_id":"  "}'), null);
    assert.equal(jobOutputJobId('builtin.job_output', '{"job_id":""}'), null);
    assert.equal(jobOutputJobId('builtin.job_output', 'not json'), null);
    assert.equal(jobOutputJobId('builtin.job_output', ''), null);
    assert.equal(jobOutputJobId('builtin.shell_command', '{"job_id":"55"}'), null);
});
