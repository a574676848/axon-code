import test from 'node:test';
import assert from 'node:assert/strict';
import builtinLabels from '../../../axon-display/src/builtin_tool_labels.json' with { type: 'json' };
import { filePathsIn } from '../src/components/conversation/filePaths.ts';
import { toolLabel, toolFailed, extractFilePath, formatToolContent, shellCommand } from '../src/components/conversation/toolPresentation.ts';
import { backendError, isBackendUnavailable } from '../src/errors.ts';
import * as ipc from '../src/ipc.ts';
import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { applyUserMessage } from '../src/conversation/transcript/cells.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import { pendingSteerCount } from '../src/conversation/transcript/derive.ts';
import { emptyTurn, refreshLiveElapsed, settleTurn } from '../src/conversation/transcript/turns.ts';

test('只把明确的文件路径变为预览入口', () => {
    const text = 'p50/p95 I/O docs/guide.md `src/main.rs` E:\\zbg\\axon-code\\Cargo.toml';
    assert.deepEqual(filePathsIn(text).map(({ path }) => path),
        ['docs/guide.md', 'src/main.rs', 'E:\\zbg\\axon-code\\Cargo.toml']);
    assert.deepEqual(filePathsIn('https://example.com/src/main.rs www.example.com/a.md'), []);
    assert.deepEqual(filePathsIn('查看 src/main.rs。以及 Cargo.toml.'),
        [{ path: 'src/main.rs', index: 3 }, { path: 'Cargo.toml', index: 18 }]);
});

test('扩展名本身就是正文用词时不能当成文件', () => {
    // 「方案 C」「用 Go 写」这类词与扩展名同名，不能渲染成文件芯片。
    assert.deepEqual(filePathsIn('方案 C 与方案 A 的区别'), []);
    assert.deepEqual(filePathsIn('用 Go 重写，顺便看看 ts 与 rs'), []);
    assert.deepEqual(filePathsIn('回「A」「B」或「C」即可开跑'), []);
    assert.deepEqual(filePathsIn('I/O 与 p50/p95 都不是路径'), []);
    // 但真正的路径照旧识别：显式形状、多级路径、裸文件名带后缀。
    assert.deepEqual(
        filePathsIn('改 ~/.axon/config.toml 与 AGENTS.md，另看 ./dist/app.js 与 .env'),
        [
            { path: '~/.axon/config.toml', index: 2 },
            { path: 'AGENTS.md', index: 24 },
            { path: './dist/app.js', index: 37 },
            { path: '.env', index: 53 },
        ],
    );
});

test('文件、便签、记忆与 MCP 的前缀独立分类', () => {
    for (const [kind, name, label] of [
        ['write_file', 'builtin.memory_write', '记忆写入'],
        ['read_file', 'builtin.notes_read_file', '读取便签'],
        ['write_file', 'notes_write_file', '写入便签'],
        ['shell', 'builtin.shell_command', '执行'],
        ['read_file', 'builtin.read_file', '读取'],
        ['mcp', 'read_file', 'MCP'],
        ['write_file', 'mcp__memory__write', 'MCP'],
    ]) assert.equal(toolLabel(kind, name), label);
    assert.equal(extractFilePath('{"file_path":"Cargo.toml","offset":1}'), 'Cargo.toml');
    assert.equal(extractFilePath('Cargo.toml'), 'Cargo.toml');
});

test('异常退出与取消标记失败，成功执行保持正常', () => {
    for (const status of ['failed', 'interrupted', 'cancelled', 'rejected'])
        assert.equal(toolFailed(status, null), true);
    assert.equal(toolFailed('completed', 2), true);
    assert.equal(toolFailed('completed', 0), false);
});

test('全部内置工具执行中和回执均使用共享中文名称', () => {
    // 共享名称表的完整性由 Rust 的真实工具注册清单覆盖测试验证。
    for (const [name, label] of Object.entries(builtinLabels)) {
        assert.equal(toolLabel('unsupported', name), label, name);
        assert.equal(toolLabel('unsupported', `builtin.${name}`), label, name);
        assert.equal(toolLabel('read_file', `builtin.${name}`), label, name);
        assert.equal(toolLabel('mcp', name), 'MCP', name);
        assert.equal(toolLabel('unsupported', `mcp__server__${name}`), 'MCP', name);
        assert.equal(toolLabel('unsupported', `plugin.${name}`), `plugin.${name}`, name);
    }
    for (const [name, label] of [
        ['update_plan', '更新计划'], ['goal_create', '创建任务'],
        ['goal_list', '列出任务'], ['goal_get', '读取任务'],
        ['goal_update', '更新任务'], ['goal_cancel', '取消任务'],
        ['delegate_to_fast', '委托快速代理'],
        ['get_context_remaining', '查看剩余上下文'], ['new_context', '新建上下文'],
    ]) assert.equal(toolLabel('unsupported', `builtin.${name}`), label);
    assert.equal(toolLabel('notes', 'builtin.notes_future'), 'builtin.notes_future');
    assert.equal(toolLabel('read_file', 'builtin.future'), 'builtin.future');
    // 任务工具的类别码随「统一内置工具中文展示」定为 task_tool；Goal 只是领域侧命名。
    assert.equal(toolLabel('task_tool', ''), '任务');
});

test('格式化 JSON 不改变数据，Shell 不改变换行和转义', () => {
    const json = '{"command":"printf \\\"a\\\\nb\\\"","cwd":"."}';
    const formatted = formatToolContent(json);
    assert.equal(formatted.language, 'json');
    assert.deepEqual(JSON.parse(formatted.code), JSON.parse(json));
    assert.equal(shellCommand(json), JSON.parse(json).command);
    const shell = 'echo "$HOME"\ncat ./file.txt\n';
    assert.equal(formatToolContent(shell).code, shell);
    assert.equal(formatToolContent('{"incomplete":').code, '{"incomplete":');
});

test('后端字符串和结构化错误保留具体原因', () => {
    assert.equal(backendError('原子写配置失败: 权限不足').message, '原子写配置失败: 权限不足');
    assert.equal(backendError({ message: '配置值非法' }).message, '配置值非法');
});

const attachment = { path: 'docs/guide.md', name: 'guide.md', image: false };
function openHistory() {
    return conversationReducer(INITIAL_CONVERSATION, { type: 'open-turn', turnId: 'history', nowMs: 0 });
}
function seed(state) {
    return conversationReducer(state, {
        type: 'seed-history', interactionId: 'history', text: '查看文件', attachments: [attachment],
    });
}

test('历史索引先到时认领占位消息，保留附件并抵抗重复事件', () => {
    let state = applyUserMessage(seed(openHistory()), 0, 'user-event', '查看文件', 100);
    state = applyUserMessage(state, 0, 'user-event', '查看文件', 100);
    assert.equal(state.turns[0].cells.length, 1);
    assert.equal(state.turns[0].index['user-event'], 0);
    assert.equal(state.turns[0].index['history-user:history'], undefined);
    assert.deepEqual(state.turns[0].cells[0].attachments, [attachment]);
});

test('历史事件先到时只补附件，不重复生成用户消息', () => {
    const state = seed(applyUserMessage(openHistory(), 0, 'user-event', '查看文件', 100));
    assert.equal(state.turns[0].cells.length, 1);
    assert.deepEqual(state.turns[0].cells[0].attachments, [attachment]);
});

test('本地消息确认建立正确索引，不同事件的相同文本仍各自保留', () => {
    let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'submit', turnId: 'local', text: '继续', nowMs: 1, attachments: [attachment],
    });
    state = applyUserMessage(state, 0, 'first', '继续', 2);
    state = applyUserMessage(state, 0, 'second', '继续', 3);
    assert.deepEqual(state.turns[0].cells.map(({ id }) => id), ['first', 'second']);
    assert.equal(state.turns[0].index.second, 1);
    assert.deepEqual(state.turns[0].cells[0].attachments, [attachment]);
});

test('带 @ 引用的消息按可见正文认领，不产生重复气泡', () => {
    const typed = '改一下 @src/model.rs';
    const submitted = `${typed}\n\n[结构化输入]\n已引用文件：src/model.rs`;
    let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'submit', turnId: 'local', text: typed, nowMs: 1, attachments: [],
    });
    state = applyUserMessage(state, 0, 'user-event', submitted, 2);

    assert.equal(state.turns[0].cells.length, 1, '服务端事件必须认领本地那一格');
    assert.equal(state.turns[0].cells[0].text, typed, '气泡只显示用户敲的那一段');
    assert.equal(state.turns[0].index['user-event'], 0);
});

test('只带图片的消息按附加块认领，气泡里仍是那一张图', () => {
    const image = { path: 'C:\\tmp\\shot.png', name: 'shot.png', image: true };
    const submitted = '[结构化输入]\n已附加文件：C:\\tmp\\shot.png';
    let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'submit', turnId: 'local', text: '', nowMs: 1, attachments: [image],
    });
    state = applyUserMessage(state, 0, 'user-event', submitted, 2);

    assert.equal(state.turns[0].cells.length, 1, '服务端事件必须认领本地那一格');
    assert.equal(state.turns[0].cells[0].text, '', '气泡里没有正文，只有附件卡');
    assert.deepEqual(state.turns[0].cells[0].attachments, [image]);
    assert.equal(state.turns[0].index['user-event'], 0);
});

test('运行中继续发送进入排队，durable 事件到达才算落地', () => {
    let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn', turnId: 'interaction-1', nowMs: 0,
    });
    state = conversationReducer(state, {
        type: 'steer', text: '顺便看一下日志', nowMs: 5, attachments: [],
    });
    assert.equal(pendingSteerCount(state.turns), 1, '未确认的消息必须计入排队');

    // 服务端在安全边界插入后按正文认领这一格：排队计数随之归零，且不出现重复气泡。
    state = applyUserMessage(state, 0, 'user-part-1', '顺便看一下日志', 6);
    assert.equal(pendingSteerCount(state.turns), 0);
    assert.deepEqual(state.turns[0].cells.map(({ id }) => id), ['user-part-1']);
});

test('轮次收口把未认领的排队消息标为未送达，而不是让它消失', () => {
    let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn', turnId: 'interaction-1', nowMs: 0,
    });
    state = conversationReducer(state, {
        type: 'steer', text: '这条没赶上当前轮', nowMs: 5, attachments: [],
    });

    // 随本轮一起被取消的消息内容是找不回来的：必须留在正文里标成未送达，
    // 用户才看得见、也点得到「重新发送」，而不是静默消失。
    const settled = settleTurn(state.turns[0], 'completed', 120);
    assert.deepEqual(settled.cells.map(({ id }) => id), ['local-steer:0-5']);
    assert.equal(settled.cells[0].undelivered, true);
    assert.equal(pendingSteerCount([settled]), 0, '收口后不再计入排队');
    assert.equal(settled.status, 'completed');
    assert.equal(settled.elapsedMs, 120);
});

test('提交失败撤销乐观轮次：不再显示生成中，消息标为未送达', () => {
    let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'submit', turnId: 'pending-1', text: '网关挂了', nowMs: 1, attachments: [],
    });
    assert.equal(state.turns[0].status, 'pending');

    state = conversationReducer(state, {
        type: 'submit-failed',
        turnId: 'pending-1',
        text: '网关挂了',
        message: '网关不可达：io: Connection refused',
        nowMs: 2,
    });

    assert.equal(state.turns[0].status, 'undelivered');
    assert.equal(state.turns[0].elapsedMs, null);
    assert.equal(state.turns[0].cells.find(({ role }) => role === 'user').undelivered, true);
    assert.ok(
        state.turns[0].cells.some(({ role, summary }) => role === 'notice' && summary.includes('网关不可达')),
        '失败原因必须原样可见',
    );
    // 关键：这一轮从未到达后端，工作状态行不能再显示它在跑。
    assert.equal(
        state.turns.some(({ status }) => status === 'pending' || status === 'streaming'),
        false,
    );
});

test('运行中排队失败只回滚那一格，不新开轮次', () => {
    let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn', turnId: 'interaction-1', nowMs: 0,
    });
    state = conversationReducer(state, {
        type: 'steer', text: '排队失败的消息', nowMs: 5, attachments: [],
    });
    state = conversationReducer(state, {
        type: 'submit-failed', turnId: null, text: '排队失败的消息', message: '网关不可达', nowMs: 6,
    });

    assert.equal(state.turns.length, 1, '排队不新开轮次，失败也不该新开');
    assert.equal(state.turns[0].status, 'streaming', '正在跑的那一轮不受影响');
    assert.equal(state.turns[0].cells[0].undelivered, true);
    assert.equal(pendingSteerCount(state.turns), 0);
});

test('提交后等待首个流式帧期间耗时照常走字', () => {
    const pending = { ...emptyTurn('turn-1', 1_000), status: 'pending' };
    assert.equal(refreshLiveElapsed(pending, 4_000).elapsedMs, 3_000);
    // 收口后的轮次不再被节拍改写：耗时以终态帧给出的结论为准。
    const completed = { ...pending, status: 'completed', elapsedMs: 200 };
    assert.equal(refreshLiveElapsed(completed, 4_000).elapsedMs, 200);
});

test('接管一条已在运行的轮次时，已耗时从服务端第一条事件起算', () => {
    const attachedAtMs = 1_000;
    const actuallyStartedAtMs = attachedAtMs - 1_200_000; // 这条轮次其实已经跑了 20 分钟
    let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn', turnId: 'interaction-live', nowMs: attachedAtMs,
    });
    state = conversationReducer(state, {
        type: 'frames',
        nowMs: attachedAtMs,
        frames: [{
            kind: 'frame',
            subscribeId: 'interaction:interaction-live',
            eventType: 'wire_event',
            payloadJson: JSON.stringify({
                schemaVersion: '1',
                eventId: 'event-1',
                correlationId: 'user',
                session: { sessionId: 'session-1' },
                sequence: 1,
                recordedAtMs: actuallyStartedAtMs,
                kind: { type: 'userMessage', payload: { partId: 'user-1', content: '很早就发出的消息' } },
            }),
            cursor: null,
            replay: true,
        }],
    });

    assert.equal(state.turns[0].startedAtMs, actuallyStartedAtMs, '起点必须让位给服务端时间');
    const ticked = refreshLiveElapsed(state.turns[0], attachedAtMs + 5_000);
    assert.equal(ticked.elapsedMs, 1_205_000, '已耗时必须包含接管之前的那段时间');
});

test('收口时停在运行中的单元一并落地，不留光标与转圈', () => {
    const turn = {
        ...emptyTurn('turn-1', 1_000),
        cells: [
            { id: 'a', role: 'assistant', text: '', streaming: true },
            { id: 'r', role: 'reasoning', text: '在想', done: false, durationMs: null, startedAtMs: 1 },
            {
                id: 't', role: 'tool', toolName: 'builtin.shell_command', toolKind: 'shell',
                command: 'ls', arguments: '{}', status: 'running', summary: '', output: '',
                durationMs: null, startedAtMs: 1, exitCode: null, jobId: null,
                fullOutputAvailable: false, filesystemWritePerformed: false, editPath: null,
                added: 0, removed: 0, diff: null,
            },
        ],
        index: { a: 0, r: 1, t: 2 },
    };

    const interrupted = settleTurn(turn, 'interrupted', 3_000);
    assert.deepEqual(interrupted.cells.map(({ id }) => id), ['a', 'r', 't'], '单元本身不该被删掉');
    assert.equal(interrupted.cells[0].streaming, false, '中断后不该再留流式光标');
    assert.equal(interrupted.cells[1].done, true, '思考要停掉转圈');
    assert.equal(interrupted.cells[2].status, 'interrupted', '没落定结果的工具行不能永远在跑');

    // 正常收口不动工具行的状态：它的结果由服务端回执给出。
    const completed = settleTurn(turn, 'completed', 3_000);
    assert.equal(completed.cells[0].streaming, false);
    assert.equal(completed.cells[1].done, true);
    assert.equal(completed.cells[2].status, 'running');
});

test('一轮里只留一根流式光标：切到工具或思考阶段就关掉', () => {
    const assistantFrame = (partId, content, delta) => ({
        kind: 'frame',
        subscribeId: 'interaction:interaction-1',
        eventType: 'wire_event',
        payloadJson: JSON.stringify({
            schemaVersion: '1',
            eventId: `assistant-${partId}-${content.length}`,
            correlationId: 'assistant',
            session: { sessionId: 'session-1' },
            sequence: 1,
            recordedAtMs: 1_000,
            kind: { type: 'assistantMessage', payload: { partId, messageId: partId, content, delta } },
        }),
        cursor: null,
        replay: false,
    });
    const toolFrame = (partId) => ({
        kind: 'frame',
        subscribeId: 'interaction:interaction-1',
        eventType: 'wire_event',
        payloadJson: JSON.stringify({
            schemaVersion: '1',
            eventId: `tool-${partId}`,
            correlationId: 'tool',
            session: { sessionId: 'session-1' },
            sequence: 2,
            recordedAtMs: 2_000,
            kind: {
                type: 'toolCall',
                payload: { partId, toolName: 'builtin.shell_command', toolKind: 'shell', summary: 'ls', argumentsSummary: '{}' },
            },
        }),
        cursor: null,
        replay: false,
    });
    const feed = (state, frames) => conversationReducer(state, { type: 'frames', nowMs: 3_000, frames });
    const streamingIds = (state) =>
        state.turns[0].cells.filter((cell) => cell.role === 'assistant' && cell.streaming).map(({ id }) => id);

    let state = conversationReducer(INITIAL_CONVERSATION, { type: 'open-turn', turnId: 'interaction-1', nowMs: 0 });
    state = feed(state, [assistantFrame('a', '第一段', true)]);
    assert.deepEqual(streamingIds(state), ['a'], '正在写的那一段有光标');

    state = feed(state, [toolFrame('t1')]);
    assert.deepEqual(streamingIds(state), [], '工具开始执行后上一段正文不该再挂光标');

    state = feed(state, [assistantFrame('b', '第二段', true)]);
    assert.deepEqual(streamingIds(state), ['b'], '新一轮正文只保留自己的光标');

    // 收尾消息（delta=false）自己就不带光标。
    state = feed(state, [assistantFrame('b', '第二段完', false)]);
    assert.deepEqual(streamingIds(state), []);
});

test('重放收尾负载里的生成指标：首字与速率不再依赖实时观测帧', () => {
    // 观测帧只走广播、不进持久日志，因此重放时首字与速率只能来自收尾负载。
    const openTurnState = conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn', turnId: 'interaction-1', nowMs: 0,
    });
    // 用量事件是 durable 的：重放时它照常存在，速率的分子就来自它。
    const usage = conversationReducer(openTurnState, {
        type: 'frames',
        nowMs: 9_000,
        frames: [{
            kind: 'frame',
            subscribeId: 'interaction:interaction-1',
            eventType: 'wire_event',
            payloadJson: JSON.stringify({
                schemaVersion: '1',
                eventId: 'usage-1',
                correlationId: 'usage',
                session: { sessionId: 'session-1' },
                sequence: 1,
                recordedAtMs: 9_000,
                kind: { type: 'usageUpdate', payload: { outputTokens: 4_600, inputTokens: 1_000 } },
            }),
            cursor: null,
            replay: true,
        }],
    });
    const completed = conversationReducer(usage, {
        type: 'frames',
        nowMs: 10_000,
        frames: [{
            kind: 'frame',
            subscribeId: 'interaction:interaction-1',
            eventType: 'turn_completed',
            payloadJson: JSON.stringify({
                response: '做完了。',
                status: 'completed',
                generation: { ttftMs: 8_460, decodeMs: 10_000 },
            }),
            cursor: null,
            replay: true,
        }],
    });
    const turn = completed.turns[0];
    assert.equal(turn.firstTokenAvgMs, 8_460, '首字来自收尾负载');
    // 出字 4600 ÷ 解码 10s = 460.0 tok/s，以十分之一为单位存储。
    assert.equal(turn.generationTpsX10, 4_600, '速率按解码窗口与出字 token 数换算');
    const summary = turn.cells.find((cell) => cell.role === 'summary');
    assert.equal(summary.firstTokenAvgMs, 8_460, '指标要回填到分割线那一行');
    assert.equal(summary.generationTpsX10, 4_600);
    // 没有观测帧时这两个数照样成立：它们不该依赖实时帧。
    assert.equal(completed.turns[0].firstTokenAvgMs !== null, true);
});

/** 造一帧 wire 事件；订阅标识与轮次标识对齐，归属规则照生产路径走。 */
function wireFrame(envelope) {
    return {
        kind: 'frame',
        subscribeId: 'interaction:interaction-live',
        eventType: 'wire_event',
        payloadJson: JSON.stringify({
            schemaVersion: '1',
            eventId: `event-${envelope.sequence ?? 1}`,
            correlationId: 'correlation-1',
            session: { sessionId: 'session-1' },
            sequence: envelope.sequence ?? 1,
            recordedAtMs: envelope.recordedAtMs ?? 1_000,
            kind: envelope.kind,
        }),
        cursor: null,
        replay: false,
    };
}

function openLiveTurn() {
    return conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn', turnId: 'interaction-live', nowMs: 0,
    });
}

test('轮次内部的阶段事件不结束整轮，窗口轮转失败只落一行历史', () => {
    // 一次 run 里 StreamingStarted / StreamingCompleted / ProviderRetry / Compaction* 会反复出现，
    // 拿它们收口会把还在执行的整轮提前判死：工作状态行消失、失败横幅常驻。
    const state = conversationReducer(openLiveTurn(), {
        type: 'frames',
        nowMs: 1_000,
        frames: [
            wireFrame({
                kind: { type: 'userMessage', payload: { partId: 'user-1', content: '整理文档' } },
            }),
            wireFrame({
                sequence: 2,
                kind: {
                    type: 'turnLifecycle',
                    payload: {
                        partId: 'lifecycle:compaction:inline:session-1',
                        turnId: 'CompactionFailed',
                        phase: 'regularTurn',
                        status: 'failed',
                        uiPhase: 'history',
                        kind: 'CompactionFailed',
                        title: '窗口轮转',
                        summary: '内联窗口轮转未完成：窗口轮转未通过门禁；当前任务历史保持不变',
                    },
                },
            }),
            wireFrame({
                sequence: 3,
                kind: {
                    type: 'turnLifecycle',
                    payload: {
                        partId: 'generation:stream:1',
                        turnId: 'provider-stream',
                        phase: 'regularTurn',
                        status: 'completed',
                        uiPhase: 'generation',
                        kind: 'StreamingCompleted',
                        title: '生成',
                    },
                },
            }),
        ],
    });

    const turn = state.turns[0];
    assert.equal(turn.status, 'streaming', '阶段事件不得把整轮收口');
    assert.equal(turn.cells.some(({ role }) => role === 'failure'), false, '不该长出失败卡');
    const history = turn.cells.find(({ role }) => role === 'history');
    assert.equal(history.title, '窗口轮转');
    assert.equal(history.status, '失败');
});

test('RunLifecycle 才是整轮终态，收口之后计划投影随之撤下', () => {
    let state = conversationReducer(openLiveTurn(), {
        type: 'frames',
        nowMs: 1_000,
        frames: [
            wireFrame({
                kind: { type: 'userMessage', payload: { partId: 'user-1', content: '整理文档' } },
            }),
            wireFrame({
                sequence: 2,
                kind: {
                    type: 'planUpdate',
                    payload: {
                        partId: 'plan:1',
                        title: '整理文档',
                        steps: [
                            { step: '审计现状', status: 'completed' },
                            { step: '改写章节', status: 'in_progress' },
                        ],
                    },
                },
            }),
        ],
    });
    assert.equal(state.turns[0].status, 'streaming');
    assert.equal(state.plan.total, 2, '运行中的计划投影照常给出');

    state = conversationReducer(state, {
        type: 'frames',
        nowMs: 2_000,
        frames: [wireFrame({
            sequence: 3,
            kind: {
                type: 'turnLifecycle',
                payload: {
                    partId: 'run:1',
                    turnId: 'run',
                    phase: 'regularTurn',
                    status: 'completed',
                    kind: 'RunLifecycle',
                    title: 'Run completed',
                },
            },
        })],
    });

    assert.equal(state.turns[0].status, 'completed', 'RunLifecycle 收口整轮');
    assert.equal(state.plan, null, '整轮跑完，固定计划坞不再占位');
    assert.equal(
        state.turns[0].cells.some(({ role }) => role === 'plan'),
        true,
        '时间线里的计划单元是每次更新的原子记录，不随投影撤下',
    );
});

test('中断结论已经在正文里时不再补同义的收尾提示', () => {
    const openTurnState = () => conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn', turnId: 'interaction-1', nowMs: 0,
    });
    const interruptedFrame = (payload) => ({
        type: 'frames',
        nowMs: 2_000,
        frames: [{
            kind: 'frame',
            subscribeId: 'interaction:interaction-1',
            eventType: 'turn_completed',
            payloadJson: JSON.stringify(payload),
            cursor: null,
            replay: false,
        }],
    });

    // 服务端已经把结论写进助手正文：正文里只应出现一次这句话。
    let stated = conversationReducer(openTurnState(), {
        type: 'frames',
        nowMs: 1_000,
        frames: [{
            kind: 'frame',
            subscribeId: 'interaction:interaction-1',
            eventType: 'wire_event',
            payloadJson: JSON.stringify({
                schemaVersion: '1',
                eventId: 'assistant-final',
                correlationId: 'assistant',
                session: { sessionId: 'session-1' },
                sequence: 2,
                recordedAtMs: 1_000,
                kind: {
                    type: 'assistantMessage',
                    payload: { partId: 'assistant:daemon-primary', messageId: 'm', content: '本轮已由用户中断。', delta: false },
                },
            }),
            cursor: null,
            replay: false,
        }],
    });
    stated = conversationReducer(stated, interruptedFrame({ response: '本轮已由用户中断。', status: 'interrupted' }));
    assert.equal(
        stated.turns[0].cells.filter(({ role }) => role === 'notice').length,
        0,
        '正文已经说过的结论不该再补一条提示',
    );
    assert.equal(
        stated.turns[0].cells.filter((cell) => (cell.text ?? '') === '本轮已由用户中断。').length,
        1,
    );

    // 没有那条正文时（中断由状态帧给出）仍然要补结论。
    const bare = conversationReducer(openTurnState(), interruptedFrame({ status: 'interrupted' }));
    assert.equal(bare.turns[0].cells.filter(({ role }) => role === 'notice').length, 1);
    assert.equal(bare.turns[0].status, 'interrupted');
});

test('只有传输层断连才判定为后端服务未启用', () => {
    assert.equal(isBackendUnavailable('网关不可达：Connection refused (os error 10061)'), true);
    assert.equal(
        isBackendUnavailable(new Error('未连接到桌面宿主，也没有检测到本地调试桥（127.0.0.1:14222）。')),
        true,
    );
    // 业务拒绝必须原样归类，否则每一个参数错误都会弹出「服务未启用」。
    assert.equal(isBackendUnavailable('网关拒绝访问：凭据缺失或不匹配'), false);
    assert.equal(isBackendUnavailable('网关不含该接口，后台服务版本可能过旧'), false);
    assert.equal(isBackendUnavailable('消息内容不能为空'), false);
});

/** 造一个调试桥响应；`rpc` 与健康探测走同一套 JSON 外壳。 */
function bridgeResponse(payload) {
    return { ok: true, status: 200, json: async () => payload };
}

/** 只在本次用例内替换调试桥，返回恢复函数。 */
function stubBridge(handler) {
    const original = globalThis.fetch;
    globalThis.fetch = async (_url, init) => {
        const request = JSON.parse(init.body);
        return bridgeResponse(handler(request.cmd, request.args));
    };
    return () => {
        globalThis.fetch = original;
    };
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test('拦截器：业务拒绝不弹窗，服务真不可用才弹窗', async () => {
    const details = [];
    const unsubscribe = ipc.onBackendUnavailable((detail) => details.push(detail));

    // 1) 业务拒绝 + 健康探测回答「已连接」：不得弹统一弹窗。
    let restore = stubBridge((cmd) =>
        cmd === 'gateway_health'
            ? { ok: true, data: { connected: true } }
            : { ok: false, error: '车道标题不能为空' });
    try {
        await assert.rejects(ipc.renameLane('lane-1', ''), /车道标题不能为空/);
        await sleep(80); // 事实核验在后台完成，等它落地再断言
        assert.deepEqual(details, [], '有服务在跑时，参数错误不能弹「服务未启用」');
    } finally {
        restore();
    }

    // 2) 业务措辞 + 健康探测回答「未连接」：事实核验命中，通知一次。
    await sleep(1300); // 越过探测节流窗口，让这一次核验真的发生
    restore = stubBridge((cmd) =>
        cmd === 'gateway_health'
            ? { ok: true, data: { connected: false } }
            : { ok: false, error: '车道不存在' });
    try {
        await assert.rejects(ipc.renameLane('lane-1', '新标题'), /车道不存在/);
        await sleep(80);
        assert.equal(details.length, 1, '判据来自探测事实，不依赖错误措辞');
        // 弹窗要说明「哪一次调用失败」，而不是只给探测结论。
        assert.match(details[0], /车道不存在/);
    } finally {
        restore();
        unsubscribe();
    }
});
