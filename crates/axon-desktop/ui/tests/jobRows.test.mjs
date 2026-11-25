import test from 'node:test';
import assert from 'node:assert/strict';
import { jobRowOf, jobElapsedMs } from '../src/conversation/transcript/derive.ts';
import { settlementAskOf } from '../src/conversation/jobRoster.ts';
import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import { activeJobCount } from '../src/conversation/runtimeFacts.ts';
import { jobDotState, jobRowConclusion, jobStatusLabel } from '../src/components/conversation/jobRowStatus.ts';

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

test('发起后台作业的行不带工具耗时：那是启动回执的测量，不是作业的', () => {
    // 启动回执几乎立刻返回，工具耗时常见为 0 秒；把它当作业耗时，收口后的行
    // 会顶着「已结束 0秒」。
    const row = rowOf({
        toolKind: 'shell',
        toolName: 'builtin.shell_command',
        durationMs: 0,
        startedAtMs: 1_000,
    });
    assert.equal(row.durationMs, null);
    assert.equal(row.startedAtMs, 1_000, '启动时刻仍要留着：在跑的行靠它走秒');
});

test('作业耗时只认实测：在跑按启动时刻走，收口用后端实测值，没有证据就不画', () => {
    const running = rowOf({
        toolKind: 'shell',
        toolName: 'builtin.shell_command',
        startedAtMs: 1_000,
        jobStatus: 'running',
    });
    // 在跑：从服务端记录的启动时刻算到这一刻，读数自己走。
    assert.equal(jobElapsedMs(running, true, 4_500, null), 3_500);
    // 收口：用后端实测值，不受采样时刻影响。
    assert.equal(jobElapsedMs(running, false, 9_999_999, 192_340), 192_340);
    // 历史清单自带实测耗时：没有收口事实时用它。
    assert.equal(jobElapsedMs({ ...running, durationMs: 61_000 }, false, 0, null), 61_000);
    // 一条证据都没有：不画，绝不退回启动回执的 0 秒。
    assert.equal(jobElapsedMs(running, false, 9_999_999, null), null);
    // 在跑却没有启动时刻：同样不画。
    assert.equal(jobElapsedMs({ ...running, startedAtMs: null }, true, 5_000, null), null);
});

test('实测 0 毫秒是读数而不是「没有证据」，坏启动时刻与倒挂采样都不画', () => {
    const row = rowOf({ toolKind: 'shell', toolName: 'builtin.shell_command', startedAtMs: 1_000 });
    // 后端实测为 0：是真实读数，不能被 `||` 或空判据当成缺失。
    assert.equal(jobElapsedMs(row, false, 0, 0), 0);
    // 启动时刻不是有限数：不拿它算耗时。
    assert.equal(jobElapsedMs({ ...row, startedAtMs: Number.NaN }, true, 5_000, null), null);
    // 采样时刻早于启动时刻（时钟回拨）：钳到 0，不显示负数。
    assert.equal(jobElapsedMs(row, true, 500, null), 0);
});

/** 台账里的一行：只填点名判定要读的字段。 */
const ledgerRow = (overrides = {}) => ({
    id: 'cell-1',
    command: 'cargo build',
    status: 'running',
    output: '',
    durationMs: null,
    startedAtMs: 1_000,
    exitCode: null,
    jobId: '7',
    jobStatus: 'running',
    fullOutputAvailable: true,
    ...overrides,
});

test('收口事实只点名「已离开名册且还没有实测耗时」的作业', () => {
    const row = ledgerRow();
    const ask = (overrides = {}) =>
        settlementAskOf({
            jobs: [row],
            live: new Set(),
            asked: new Set(),
            ...overrides,
        });

    // 正常该问：名册已不再持有、又没有实测耗时。
    assert.deepEqual(ask().ids, ['7']);
    assert.deepEqual(ask().rows.map((item) => item.id), ['cell-1']);
    // 还在名册里：它在跑，耗时由界面按秒走。
    assert.deepEqual(ask({ live: new Set(['7']) }).ids, []);
    // 已经问过：不再重复发请求。
    assert.deepEqual(ask({ asked: new Set(['7']) }).ids, []);
    // 历史清单自带实测耗时：不重复问后端。
    assert.deepEqual(ask({ jobs: [ledgerRow({ durationMs: 61_000 })] }).ids, []);
    // 前台命令与交互式会话没有作业标识，不进这张面板的点名。
    assert.deepEqual(ask({ jobs: [ledgerRow({ jobId: null })] }).ids, []);
});

test('作业标识重用只认最后一行，超出上限的留给下一拍', () => {
    // 进程重启后标识从 1 重计：同一个 7 同时对应上一代的死行与这一代的活行。
    const stale = ledgerRow({ id: 'cell-old', jobId: '7' });
    const current = ledgerRow({ id: 'cell-new', jobId: '7', startedAtMs: 9_000 });
    const asked = new Set();
    const first = settlementAskOf({ jobs: [stale, current], live: new Set(), asked });
    assert.deepEqual(first.ids, ['7']);
    assert.deepEqual(first.rows.map((row) => row.id), ['cell-new']);

    // 上限截断：只发前两条，没发出去的不许标记成已问过；从新到旧挑，
    // 用户刚看完的那几条先拿到读数。
    const many = ['1', '2', '3'].map((jobId) => ledgerRow({ id: `cell-${jobId}`, jobId }));
    const limited = settlementAskOf({ jobs: many, live: new Set(), asked: new Set(), limit: 2 });
    assert.deepEqual(limited.ids, ['3', '2']);
    assert.equal(limited.rows.length, 2);
    // 下一拍接着补：已问过的排除后只剩最旧那一条。
    const next = settlementAskOf({
        jobs: many,
        live: new Set(),
        asked: new Set(['3', '2']),
    });
    assert.deepEqual(next.ids, ['1']);
});

test('六个终态词都有中文文案，异常终态共用错误档', () => {
    assert.equal(jobStatusLabel('completed'), '已完成');
    assert.equal(jobStatusLabel('failed'), '已失败');
    assert.equal(jobStatusLabel('killed'), '已取消');
    assert.equal(jobStatusLabel('timeout'), '已超时');
    assert.equal(jobStatusLabel('unresponsive'), '已失去响应');
    assert.equal(jobStatusLabel('refusal'), '已拒绝执行');
    // 超时、失去响应与被拒绝执行都算「没有正常收口」：与 failed 同档，不落灰点。
    for (const status of ['failed', 'timeout', 'unresponsive', 'refusal']) {
        assert.equal(jobDotState(status, { running: false, stopped: false }), 'error');
    }
});

test('行尾结论：干净退出不画退出码，异常终态取危险色，后端已不再持有写已结束', () => {
    const conclusion = (overrides = {}) =>
        jobRowConclusion({
            status: 'completed',
            exitCode: 0,
            live: false,
            stoppedReason: null,
            ...overrides,
        });

    // 干净退出：只写状态词，不画 exit 0。
    assert.equal(conclusion().text, '已完成');
    assert.equal(conclusion().danger, false);
    // 非零退出：画退出码并取危险色。
    assert.equal(conclusion({ exitCode: 3 }).text, 'exit 3');
    assert.equal(conclusion({ exitCode: 3 }).danger, true);
    // 异常终态即使没有退出码，也要中文结论加危险色。
    assert.equal(conclusion({ status: 'timeout', exitCode: null }).text, '已超时');
    assert.equal(conclusion({ status: 'timeout', exitCode: null }).danger, true);
    // 名册已不再持有、状态词却还写着在跑：如实写已结束，并标记为失效行。
    assert.equal(conclusion({ status: 'running', exitCode: null }).text, '已结束');
    assert.equal(conclusion({ status: 'running', exitCode: null }).stale, true);
    // 用户手动停止的原因最具体，压过状态词。
    assert.equal(
        conclusion({ status: 'failed', exitCode: null, live: true, stoppedReason: '已停止' }).text,
        '已停止',
    );
    // 确实在跑：写运行中，且不取危险色。
    assert.equal(conclusion({ status: 'running', exitCode: null, live: true }).text, '运行中');
    assert.equal(conclusion({ status: 'running', exitCode: null, live: true }).danger, false);
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
