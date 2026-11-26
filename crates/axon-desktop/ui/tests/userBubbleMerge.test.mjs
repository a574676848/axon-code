import test from 'node:test';
import assert from 'node:assert/strict';

import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import { projectFrames, wireFrame } from '../src/conversation/historyProjection.ts';
import { provisionalFrames } from '../src/conversation/pagingSkeleton.ts';
import { orderTurnCells } from '../src/conversation/transcript/order.ts';

/*
 * 同一条消息只能有一格气泡。
 *
 * 运行中排队时本地乐观格用 `local-steer:` 立起并标着「等待注入」；索引重载又会为同一
 * 交互立起 `local-user:` 骨架。durable 用户帧到达时只认领第一格，另一格原地留下，
 * 现场因此看到「一条没有标记、一条还在等待注入」的两格气泡。这里钉住合并后的形态。
 */

const RUNNING = '01RUNNINGINTERACTION00000000';
const QUEUED = '01QUEUEDINTERACTION000000000';
const VISIBLE = '改一下文案';
const STRUCTURED = '[结构化输入]\n已引用文件：src/a.ts';

/** 一条 durable 用户帧；结构化输入块按提交时的原样接在正文之后。 */
function userFrame(partId, content, interactionId) {
    return wireFrame(interactionId, 3_000, 'userMessage', {
        partId,
        messageId: `${partId}:message`,
        content,
    });
}

/** 一条轮次收口帧：运行结束，本轮不再等待注入。 */
function completedFrameOf(interactionId) {
    return {
        kind: 'frame',
        subscribeId: `interaction:${interactionId}`,
        eventType: 'turn_completed',
        cursor: null,
        payloadJson: JSON.stringify({ interactionId, response: '好', status: 'completed' }),
    };
}

/**
 * 一条带归属交互标识的 durable 用户帧。
 *
 * 与 `userFrame` 的区别是信封自带 `interactionId`：服务端复用同一条 Interaction 把这条
 * 消息注入回当时那一轮（而不是另开一轮），迟到的它就该落回原轮，认领那里留下的乐观格。
 */
function userFrameInTurn(partId, content, interactionId) {
    return {
        kind: 'frame',
        subscribeId: `interaction:${interactionId}`,
        eventType: 'wire_event',
        cursor: null,
        payloadJson: JSON.stringify({
            schemaVersion: 'axon.wire.v3',
            eventId: `${partId}:event`,
            correlationId: 'user',
            interactionId,
            session: { sessionId: 'session-merge' },
            sequence: 1,
            recordedAtMs: 4_000,
            kind: { type: 'userMessage', payload: { partId, content } },
        }),
    };
}

/** 开一条正在跑的轮次，并落下它的开场输入。 */
function runningTurn() {
    let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn',
        turnId: RUNNING,
        nowMs: 1_000,
    });
    state = conversationReducer(state, {
        type: 'frames',
        frames: [userFrame('user:opening', '先读一下现状', RUNNING)],
        nowMs: 1_000,
    });
    return state;
}

/** 运行中排队：本地乐观格标着「等待注入」，并带着提交时的结构化输入。 */
function steer(state, text, structuredInput = '', attachments = []) {
    return conversationReducer(state, {
        type: 'steer',
        text,
        nowMs: 2_000,
        attachments,
        structuredInput,
    });
}

/** 索引重载为这条排队消息立起的骨架轮（状态 received 表示还没被接手）。 */
function skeletonTurn(interactionId, content) {
    let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn',
        turnId: interactionId,
        nowMs: 2_000,
    });
    state = conversationReducer(state, {
        type: 'frames',
        frames: provisionalFrames({
            interactionId,
            receivedAtMs: 2_000,
            status: 'received',
            content,
        }),
        nowMs: 2_000,
    });
    return state.turns[0];
}

/** 投影里所有可见正文等于 text 的用户格。 */
function bubbles(state, text) {
    const found = [];
    for (const turn of state.turns) {
        for (const cell of turn.cells) {
            if (cell.role === 'user' && cell.text.trim() === text) found.push({ turn, cell });
        }
    }
    return found;
}

test('queued_steer_and_skeleton_cell_merge_into_one：排队消息与索引骨架合并成一格', () => {
    let state = steer(runningTurn(), VISIBLE);
    assert.equal(bubbles(state, VISIBLE).length, 1, '排队时先只有本地乐观格');
    assert.equal(bubbles(state, VISIBLE)[0].cell.id.startsWith('local-steer:'), true);
    // 索引重载：服务端已经把这条排队消息登记成一条交互，骨架另立一轮。
    state = conversationReducer(state, {
        type: 'append-turns',
        turns: [skeletonTurn(QUEUED, VISIBLE)],
    });
    assert.equal(state.turns.length, 2, '骨架是一轮，乐观格仍在上一轮');
    assert.equal(bubbles(state, VISIBLE).length, 2, '这是缺陷的起点：两格同文本气泡');

    // durable 用户帧到达：它落在最后那条还在跑的轮次上。
    state = conversationReducer(state, {
        type: 'frames',
        frames: [userFrame('user:queued', VISIBLE, QUEUED)],
        nowMs: 3_000,
    });

    const merged = bubbles(state, VISIBLE);
    assert.equal(merged.length, 1, '同一条消息只能留一格气泡');
    assert.equal(merged[0].cell.id, 'user:queued', '留下的那一格用服务端标识');
    assert.equal(merged[0].cell.steer, true, '排队这件事跟着这一格走，认领不改变它的落位语义');
    assert.equal(typeof merged[0].cell.steerSeq, 'number', '到达顺序只认最早那次用户动作');
    // 上一轮里的乐观格必须摘掉，否则它仍挂着「等待注入」。
    const previous = state.turns.find((turn) => turn.id === RUNNING);
    assert.equal(
        previous.cells.some((cell) => cell.id.startsWith('local-steer:')),
        false,
    );
    assert.equal(orderTurnCells(merged[0].turn.cells).length, 1, '轮内顺序仍由 orderTurnCells 决定');
});

test('structured_input_message_renders_single_bubble：结构化输入只留一格气泡', () => {
    // 同一轮里同时存在乐观格与骨架格：先认领再合并，两格归一。
    let state = steer(runningTurn(), VISIBLE, STRUCTURED);
    const turn = state.turns[0];
    const doubled = {
        ...turn,
        cells: [
            ...turn.cells,
            { id: `local-user:${RUNNING}`, role: 'user', text: VISIBLE, atMs: null, attachments: [] },
        ],
    };
    doubled.index = Object.fromEntries(doubled.cells.map((cell, index) => [cell.id, index]));
    state = conversationReducer(state, { type: 'replace-turn', turn: doubled });
    assert.equal(bubbles(state, VISIBLE).length, 2);

    state = conversationReducer(state, {
        type: 'frames',
        frames: [userFrame('user:structured', `${VISIBLE}\n\n${STRUCTURED}`, RUNNING)],
        nowMs: 3_000,
    });

    const merged = bubbles(state, VISIBLE);
    assert.equal(merged.length, 1, '结构化输入块不许把同一条消息拆成两格');
    assert.equal(merged[0].cell.id, 'user:structured');
    assert.equal(merged[0].cell.text, VISIBLE, '气泡只显示剥掉结构化输入块之后的正文');
    assert.equal(
        merged[0].cell.structuredInput,
        STRUCTURED,
        '结构化输入要留住：中断转投要按原样重发',
    );
    assert.equal(state.turns[0].cells.length, 2, '开场输入与这条消息各一格，没有第三格');
});

/*
 * 服务端把这条追加消息落到**另一条 Interaction** 上时的两格气泡。
 *
 * 服务端没有把它注入当时那一轮，而是另开一轮接手，那条轮次整轮从索引与帧投影进来
 * （`append-turns` / `replace-turn`），途中不经过 `applyUserMessage` 的按正文认领。
 * 上一轮的乐观格因此会原地留下，收口时还被标成「未送达」，现场读作同一句话出现了
 * 两遍（一条未送达、一条带来源标签）。
 */

test('index_projected_turn_adopts_queued_steer：整轮按帧投影接管时收掉上一轮的乐观占位', () => {
    let state = steer(runningTurn(), VISIBLE);
    // 索引先为这条消息立起骨架轮：骨架用的是本地标识，此时还认领不了。
    state = conversationReducer(state, {
        type: 'append-turns',
        turns: [skeletonTurn(QUEUED, VISIBLE)],
    });
    assert.equal(bubbles(state, VISIBLE).length, 2, '骨架轮只带本地标识，认领留到帧到达');

    // 帧读回：整轮替换成按帧投影的形态，用户消息带服务端标识与逐轮来源。
    const projected = projectFrames(
        QUEUED,
        [userFrame('user:queued', VISIBLE, QUEUED)],
        { attachments: [], channel: 'desktop' },
        undefined,
        null,
        { settled: false },
    );
    state = conversationReducer(state, {
        type: 'replace-turn',
        turn: projected.turn,
        jobs: projected.jobs,
    });

    const merged = bubbles(state, VISIBLE);
    assert.equal(merged.length, 1, '同一条消息只能留一格气泡');
    assert.equal(merged[0].cell.id, 'user:queued');
    assert.equal(merged[0].cell.channel, 'desktop', '留下的是服务端那一格，来源标签来自索引');
    const previous = state.turns.find((turn) => turn.id === RUNNING);
    assert.equal(
        previous.cells.some((cell) => cell.id.startsWith('local-steer:')),
        false,
        '上一轮的乐观格必须摘掉，否则收口后会挂着未送达',
    );
    assert.equal(previous.cells.length, 1, '上一轮的开场输入仍然留着');
});

test('late_frame_opens_next_turn_without_second_bubble：服务端把追加消息落到下一轮时不留第二格', () => {
    let state = steer(runningTurn(), VISIBLE);
    // 这一轮跑完：还没被认领的追加消息按既有规则标成未送达，留在正文里作为证据。
    state = conversationReducer(state, {
        type: 'frames',
        frames: [completedFrameOf(RUNNING)],
        nowMs: 2_500,
    });
    assert.equal(bubbles(state, VISIBLE)[0].cell.undelivered, true, '先按未送达留证');

    // 服务端随后把这条消息作为下一轮的开场发下来：它必须收走上一轮那一格。
    state = conversationReducer(state, {
        type: 'frames',
        frames: [userFrame('user:next', VISIBLE, QUEUED)],
        nowMs: 3_000,
    });

    const merged = bubbles(state, VISIBLE);
    assert.equal(merged.length, 1, '同一条消息只能留一格气泡');
    assert.equal(merged[0].cell.id, 'user:next');
    assert.equal(state.turns.length, 2, '两轮各在其位：上一轮的开场输入与新的一轮');
});

test('claim_clears_stale_undelivered：认领即已送达，不继承未送达结论', () => {
    let state = steer(runningTurn(), VISIBLE);
    // 这一轮先收口：还在等注入的乐观格按既有规则标成未送达。
    state = conversationReducer(state, {
        type: 'frames',
        frames: [completedFrameOf(RUNNING)],
        nowMs: 2_500,
    });
    assert.equal(bubbles(state, VISIBLE)[0].cell.undelivered, true, '收口那一刻确实还没被认领');

    // 服务端随后把它注入并回放出来：同一条消息，同一个 Interaction。
    state = conversationReducer(state, {
        type: 'frames',
        frames: [userFrameInTurn('user:late', VISIBLE, RUNNING)],
        nowMs: 4_000,
    });

    const merged = bubbles(state, VISIBLE);
    assert.equal(merged.length, 1, '认领之后仍然只有一格气泡');
    assert.equal(merged[0].cell.id, 'user:late', '标识换成服务端那一份');
    assert.equal(
        merged[0].cell.undelivered,
        undefined,
        '认领本身就是已送达的证据，未送达标记必须撤下',
    );
});

/*
 * 附件卡与气泡是同一格的两部分，但附件的唯一事实留在正文里。
 *
 * wire 帧不承载附件、历史索引只记这一轮开场那一份，运行中追加的那一格两边都拿不到：
 * 附件只以 `已附加文件：<路径>` 的形式躺在结构化输入块里。展示层剥掉块之后若不反解
 * 回来，气泡会留着而附件卡消失（现场：气泡还在，「image.png」那张卡没了）。
 */

const SHOT = '/fixtures/screenshot.png';
const SHOT_BLOCK = `[结构化输入]\n已附加文件：${SHOT}`;

test('attachments_survive_content_roundtrip：附件由正文反解，认领到别的轮次也不丢', () => {
    const visible = 'oms账号密码';
    // 现场：运行中追加，本地那份带着真实附件事实。
    let state = steer(runningTurn(), visible, '', [
        { path: SHOT, name: 'screenshot.png', image: true, bytes: 2_048 },
    ]);
    assert.equal(state.turns[0].cells.at(-1).attachments.length, 1);

    // 服务端另开一轮接手：durable 正文带着结构化输入块，附件只剩那几行文字。
    const projected = projectFrames(
        QUEUED,
        [userFrame('user:queued', `${visible}\n\n${SHOT_BLOCK}`, QUEUED)],
        { attachments: [], channel: 'desktop' },
        undefined,
        null,
        { settled: false },
    );
    state = conversationReducer(state, { type: 'append-turns', turns: [projected.turn] });

    const merged = bubbles(state, visible);
    assert.equal(merged.length, 1, '同一条消息只能留一格气泡');
    assert.equal(merged[0].cell.text, visible, '结构化输入块不进气泡正文');
    assert.deepEqual(
        merged[0].cell.attachments.map((item) => item.path),
        [SHOT],
        '附件卡由正文里的附件行还原，不能因为认领换了标识就丢掉',
    );
});

test('attachment_only_message_keeps_its_card：只有附件没有正文时，卡还在、气泡不空转', () => {
    // 只有附件时结构化输入块承担整条正文，可见正文是空串。
    const projected = projectFrames(
        QUEUED,
        [userFrame('user:only', SHOT_BLOCK, QUEUED)],
        undefined,
        undefined,
        null,
        { settled: false },
    );
    const state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'prepend-turns',
        turns: [projected.turn],
    });

    const cell = state.turns[0].cells.find((item) => item.role === 'user');
    assert.equal(cell.text, '', '整条正文就是这个块，气泡不显示它');
    assert.deepEqual(cell.attachments.map((item) => item.path), [SHOT], '附件卡必须还在');
});

test('never_empties_a_turn：宁可留一格气泡，也不造一个永远在跑的空轮', () => {
    // 这一轮只有那一格乐观占位：摘掉它会让整轮空掉，此后没有任何帧再推进它。
    let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn',
        turnId: RUNNING,
        nowMs: 1_000,
    });
    state = steer(state, VISIBLE);
    assert.equal(state.turns[0].cells.length, 1);

    const projected = projectFrames(
        QUEUED,
        [userFrame('user:queued', VISIBLE, QUEUED)],
        undefined,
        undefined,
        null,
        { settled: false },
    );
    state = conversationReducer(state, {
        type: 'append-turns',
        turns: [projected.turn],
    });

    const previous = state.turns.find((turn) => turn.id === RUNNING);
    assert.equal(previous.cells.length, 1, '被摘空的那一轮会以运行中永远留在时间线上，因此不摘');
});

test('claimed_message_clears_pending_attachments：认领后附件暂存必须清空', () => {
    let state = steer(runningTurn(), VISIBLE);
    const turn = state.turns[0];
    // 索引先把附件记在轮次上：用户消息还没重放出来，暂存等它落地。
    const withPending = {
        ...turn,
        pendingAttachments: [
            { path: '/tmp/pasted-notes.md', name: 'pasted-notes.md', image: false, bytes: 20480 },
        ],
    };
    state = conversationReducer(state, { type: 'replace-turn', turn: withPending });
    assert.equal(state.turns[0].pendingAttachments.length, 1, '构造出待贴的附件');

    state = conversationReducer(state, {
        type: 'frames',
        frames: [userFrame('user:with-attachment', VISIBLE, RUNNING)],
        nowMs: 3_000,
    });

    assert.equal(bubbles(state, VISIBLE).length, 1, '认领后仍只有一格气泡');
    assert.equal(
        state.turns[0].pendingAttachments.length,
        0,
        '认领之后附件暂存必须清空，否则同一轮后面的消息会再挂一张重复附件卡',
    );
});
