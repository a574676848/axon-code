import test from 'node:test';
import assert from 'node:assert/strict';
import { deriveJobs } from '../src/conversation/transcript/derive.ts';

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
