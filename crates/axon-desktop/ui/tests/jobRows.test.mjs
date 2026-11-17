import test from 'node:test';
import assert from 'node:assert/strict';
import { deriveJobs } from '../src/conversation/transcript/derive.ts';
import { activeJobCount } from '../src/conversation/runtimeFacts.ts';
import { jobDotState } from '../src/components/conversation/jobRowStatus.ts';

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

test('前台 shell 调用与交互式会话不进后台任务列表', () => {
    // 只有回执带回 job_id 的调用才是后台作业：前台命令与 tty 会话给的是 session_id，
    // 把它们也列进这张面板，会让「后台任务 N」的读数与事实对不上。
    const foreground = jobsOf(
        toolCell({
            id: 'foreground',
            toolKind: 'shell',
            toolName: 'builtin.shell_command',
            command: 'git -C repo status --short',
            exitCode: 0,
            jobId: null,
        }),
    );
    assert.equal(foreground.length, 0);

    const background = jobsOf(
        toolCell({
            id: 'background',
            toolKind: 'shell',
            toolName: 'builtin.shell_command',
            command: 'git -C repo switch --create master --track origin/master',
            jobId: '1',
        }),
    );
    assert.deepEqual(background.map((job) => job.id), ['background']);
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

test('状态点只在作业确实在跑时转圈', () => {
    // 状态词是转录里的历史快照：名册已不再持有它时，行内文案已经写「已结束」，
    // 状态点不能还挂着那一枚转圈，否则收口的作业会永远转下去。
    assert.equal(jobDotState('running', { running: false, stopped: false }), 'idle');
    // 确实在跑：只有这一档才转。
    assert.equal(jobDotState('running', { running: true, stopped: false }), 'ongoing');
    // 本轮被用户停下、状态词还没翻过来：与 killed 同档，共用警示色。
    assert.equal(jobDotState('running', { running: false, stopped: true }), 'warning');
    // 终态各自的状态点不受活动事实影响，正在停止也仍是琥珀而不是转圈。
    assert.equal(jobDotState('completed', { running: false, stopped: false }), 'done');
    assert.equal(jobDotState('failed', { running: false, stopped: false }), 'error');
    assert.equal(jobDotState('killed', { running: false, stopped: false }), 'warning');
    assert.equal(jobDotState('stopping', { running: true, stopped: false }), 'warning');
    // 排队与未知状态落在静息灰点。
    assert.equal(jobDotState('pending', { running: false, stopped: false }), 'idle');
});