import test from 'node:test';
import assert from 'node:assert/strict';
import { deriveJobs } from '../src/conversation/transcript/derive.ts';
import { activeJobCount } from '../src/conversation/runtimeFacts.ts';

/** 工具单元的最小事实：派生作业行只读这些字段。 */
const toolCell = (overrides = {}) => ({
    id: 'cell-1',
    role: 'tool',
    toolKind: 'job',
    toolName: 'builtin.job_output',
    command: '{"job_id":"6"}',
    summary: '',
    status: 'completed',
    output: '',
    durationMs: 34607,
    startedAtMs: 0,
    exitCode: null,
    jobId: '6',
    jobStatus: null,
    fullOutputAvailable: true,
    ...overrides,
});

const jobsOf = (...cells) => deriveJobs({ turns: [{ cells }] });

test('作业读取工具本身不是后台任务行', () => {
    // 读取工具没有退出码，也不是作业本身：列进「后台任务」会重复出现在同一作业下，
    // 并被判成「未正常退出」。
    assert.equal(jobsOf(toolCell()).length, 0);
    assert.equal(jobsOf(toolCell({ toolName: 'builtin.job_kill' })).length, 0);
});

test('发起作业的 shell 调用仍然留在后台任务列表里', () => {
    const jobs = jobsOf(
        toolCell({
            toolKind: 'shell',
            toolName: 'builtin.shell_command',
            command: 'pip install scrapling',
            exitCode: 0,
        }),
    );
    assert.equal(jobs.length, 1);
    assert.equal(jobs[0].command, 'pip install scrapling');
    assert.equal(jobs[0].jobId, '6');
});

test('一次作业只贡献一行，读取若干次不会把它复制成多行', () => {
    const jobs = jobsOf(
        toolCell({ id: 'start', toolKind: 'shell', toolName: 'builtin.shell_command', command: 'pip install scrapling' }),
        toolCell({ id: 'poll-1', command: '{"job_id":"6"}' }),
        toolCell({ id: 'poll-2', command: '{"job_id":"6","yield_time_ms":120000}' }),
    );
    assert.deepEqual(jobs.map((job) => job.id), ['start']);
});

test('行状态取作业自身状态，启动回执不会让行永远显示运行中', () => {
    // 启动回执的调用状态是 completed，但作业还在跑：行必须显示 running。
    const running = jobsOf(
        toolCell({
            status: 'completed',
            jobStatus: 'running',
            toolKind: 'shell',
            toolName: 'builtin.shell_command',
            command: 'cargo build',
        }),
    );
    assert.equal(running[0].status, 'running');

    // 作业真正收口后，行随之变成终态，不再依赖名册对账。
    const done = jobsOf(
        toolCell({
            status: 'completed',
            jobStatus: 'completed',
            toolKind: 'shell',
            toolName: 'builtin.shell_command',
            command: 'cargo build',
        }),
    );
    assert.equal(done[0].status, 'completed');
});

test('没有 jobStatus 的普通工具行仍然用调用状态', () => {
    const jobs = jobsOf(
        toolCell({ toolKind: 'shell', toolName: 'builtin.shell_command', command: 'curl -sI', status: 'completed', jobStatus: null }),
    );
    assert.equal(jobs[0].status, 'completed');
});

test('进行中计数以活作业名册为权威，启动快照不会把已收口的任务算成在跑', () => {
    const rows = [
        { status: 'running', jobId: '39' },
        { status: 'running', jobId: '40' },
        { status: 'completed', jobId: '38' },
    ];

    // 名册里只剩 40：39 已经收口，只是启动快照还写着 running。
    assert.equal(activeJobCount(rows, new Set(['40'])), 1);
    // 还没对过账时退回状态判断，宁可多算一次也不误报「没有任务」。
    assert.equal(activeJobCount(rows, null), 2);
    // 没有 jobId 的前台命令只能用状态判断。
    assert.equal(activeJobCount([{ status: 'running', jobId: null }], new Set()), 1);
    // 名册清空即全部收口。
    assert.equal(activeJobCount(rows, new Set()), 0);
});