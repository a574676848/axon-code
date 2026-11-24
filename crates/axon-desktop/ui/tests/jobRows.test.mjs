import test from 'node:test';
import assert from 'node:assert/strict';
import { jobRowOf } from '../src/conversation/transcript/derive.ts';
import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import { activeJobCount } from '../src/conversation/runtimeFacts.ts';
import { jobDotState } from '../src/components/conversation/jobRowStatus.ts';

/** 工具单元的最小事实：作业行只读这些字段。 */
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

const rowOf = (overrides) => jobRowOf(toolCell(overrides));

test('作业读取工具本身不是后台任务行', () => {
    // 读取工具没有退出码，也不是作业本身：列进「后台任务」会重复出现在同一作业下，
    // 并被判成「未正常退出」。
    assert.equal(rowOf(), null);
    assert.equal(rowOf({ toolName: 'builtin.job_kill' }), null);
});

test('发起作业的 shell 调用仍然留在后台任务列表里', () => {
    const row = rowOf({
        toolKind: 'shell',
        toolName: 'builtin.shell_command',
        command: 'pip install scrapling',
        exitCode: 0,
    });
    assert.equal(row.command, 'pip install scrapling');
    assert.equal(row.jobId, '6');
});

test('行状态取作业自身状态，启动回执不会让行永远显示运行中', () => {
    // 启动回执的调用状态是 completed，但作业还在跑：行必须显示 running。
    assert.equal(
        rowOf({ jobStatus: 'running', toolKind: 'shell', toolName: 'builtin.shell_command' }).status,
        'running',
    );
    // 作业真正收口后，行随之变成终态，不再依赖名册对账。
    assert.equal(
        rowOf({ jobStatus: 'completed', toolKind: 'shell', toolName: 'builtin.shell_command' }).status,
        'completed',
    );
    // 没有 jobStatus 的普通工具行仍然用调用状态。
    assert.equal(
        rowOf({ toolKind: 'shell', toolName: 'builtin.shell_command', status: 'completed' }).status,
        'completed',
    );
});

test('前台 shell 调用与交互式会话不进后台任务列表', () => {
    // 只有回执带回 job_id 的调用才是后台作业：前台命令与 tty 会话给的是 session_id。
    assert.equal(rowOf({ toolKind: 'shell', toolName: 'builtin.shell_command', jobId: null }), null);
});

/** 一帧 wire 事件。 */
function wire(sequence, kind, payload) {
    return {
        kind: 'frame',
        subscribeId: 'interaction:i1',
        eventType: 'wire_event',
        cursor: `cur-${sequence}`,
        payloadJson: JSON.stringify({
            schemaVersion: 'axon.wire.v3',
            eventId: `e${sequence}`,
            correlationId: 'job',
            session: { sessionId: 's1' },
            sequence,
            recordedAtMs: 1_000 + sequence,
            kind: { type: kind, payload },
        }),
    };
}

/** 一轮后台作业的帧：发起 shell、回执带 jobId、正文与终态。 */
function jobTurnFrames() {
    return [
        wire(1, 'userMessage', { partId: 'u1', messageId: 'm1', content: '后台跑一个 job' }),
        wire(2, 'toolCall', {
            partId: 't1',
            toolCallId: 'call1',
            toolName: 'shell_command',
            toolKind: 'shell',
            summary: 'cargo build',
            argumentsSummary: '{"command":"cargo build"}',
        }),
        wire(3, 'toolResult', {
            partId: 't1',
            toolCallId: 'call1',
            status: 'completed',
            summary: 'job 2 started',
            outputPreview: 'job start job_id=2',
            metadata: { jobId: '2', jobStatus: 'running' },
        }),
        wire(4, 'assistantMessage', { partId: 'a1', messageId: 'm2', content: '已启动', delta: false }),
    ];
}

test('作业记进会话台账，折叠释放过程单元之后清单仍在', () => {
    const frames = jobTurnFrames();
    const opened = conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn',
        turnId: 'i1',
        nowMs: 990,
    });
    let state = conversationReducer(opened, { type: 'frames', frames, nowMs: 5_000 });
    assert.equal(state.jobs.length, 1);
    assert.equal(state.jobs[0].jobId, '2');
    assert.equal(state.jobs[0].status, 'running');

    // 折叠把工具单元整段撤下：台账必须原样留着，否则标题栏的后台任务清单会变空。
    state = conversationReducer(state, { type: 'fold-turn', turnId: 'i1' });
    assert.equal(state.turns[0].cells.some((cell) => cell.role === 'tool'), false);
    assert.equal(state.jobs.length, 1);
    assert.equal(state.jobs[0].id, 't1');
});

test('翻页带过来的作业台账与现有台账合并，同一条作业不会出现两行', () => {
    const frames = jobTurnFrames();
    const opened = conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn',
        turnId: 'i1',
        nowMs: 990,
    });
    const live = conversationReducer(opened, { type: 'frames', frames, nowMs: 5_000 });
    const merged = conversationReducer(live, {
        type: 'append-turns',
        turns: live.turns,
        jobs: [{ ...live.jobs[0], status: 'completed' }],
    });
    assert.equal(merged.turns.length, 1, '同一轮不会重复接入');
    assert.equal(merged.jobs.length, 1);
    assert.equal(merged.jobs[0].status, 'completed', '后到的回执覆盖先到的快照');
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
