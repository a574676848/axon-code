import test from 'node:test';
import assert from 'node:assert/strict';

import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import { wireFrame } from '../src/conversation/historyProjection.ts';
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
function steer(state, text, structuredInput = '') {
    return conversationReducer(state, {
        type: 'steer',
        text,
        nowMs: 2_000,
        attachments: [],
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
