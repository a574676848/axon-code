import test from 'node:test';
import assert from 'node:assert/strict';

import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import { pendingSteerCount } from '../src/conversation/transcript/derive.ts';
import { subscribedInteractionId } from '../src/conversation/wire.ts';
import { wireFrame } from '../src/conversation/historyProjection.ts';

/*
 * 交互状态帧的归属。
 *
 * 运行中追加的消息在服务端同样是一条 Interaction：它被注入活跃回合之后状态即为
 * `understood`，网关回放会为它补一条状态帧（`gateway_replay.rs` 的终态补帧）。
 * 那一帧说的是「这条消息送进去了」，不是「那一轮跑完了」。
 *
 * 按「最后一条在跑的轮次」落帧，正在流式输出的一轮会被当场收口：`working` 变假，
 * 输入坞的工作状态行（供应商/模型与「另有 N 条等待注入」）连同中断入口一起消失，
 * 而正文其实还在长，并且没有任何一帧会把它改回来。这里钉住归属规则：状态帧只落
 * 它自己那条轮次，对不上任何轮次的帧不改变投影。
 */

const RUNNING = '01RUNNINGINTERACTION00000000';
const STEER = '01STEERINTERACTION0000000000';
const STEER_TEXT = '顺便看一下日志';

/** 一条状态帧：载荷只有一个状态字符串，归属只写在订阅标识上。 */
function statusFrameOf(interactionId, status) {
    return {
        kind: 'frame',
        subscribeId: `interaction:${interactionId}`,
        eventType: 'interaction.status',
        cursor: null,
        payloadJson: JSON.stringify(status),
    };
}

/** 一条已经绑定交互标识、正在流式输出、且已落下开场输入的轮次。 */
function streamingTurn() {
    let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'submit',
        turnId: 'pending-1000',
        text: '先读一下现状',
        nowMs: 1_000,
        attachments: [],
    });
    state = conversationReducer(state, {
        type: 'bind',
        turnId: 'pending-1000',
        interactionId: RUNNING,
    });
    state = conversationReducer(state, {
        type: 'frames',
        frames: [wireFrame(RUNNING, 2_000, 'providerStatus', {
            provider: 'sensenova',
            model: 'x',
            phase: 'generating',
        })],
        nowMs: 2_000,
    });
    return state;
}

/** 运行中追加：这一格是本地乐观格，标着「等待注入」。 */
function queuedSteer(state) {
    return conversationReducer(state, {
        type: 'steer',
        text: STEER_TEXT,
        nowMs: 3_000,
        attachments: [],
        structuredInput: '',
    });
}

/** 界面是否仍认为这一轮在跑：与 `useConversation` 的 `working` 同一判据。 */
function working(state) {
    return state.turns.some((turn) => turn.status === 'pending' || turn.status === 'streaming');
}

test('foreign_status_frame_keeps_running_turn：注入回执不把正在跑的轮次收口', () => {
    let state = queuedSteer(streamingTurn());
    assert.equal(working(state), true, '追加一条消息之后本轮仍在跑');
    assert.equal(pendingSteerCount(state.turns), 1, '这一条还在等安全边界注入');

    // 服务端把这条追加消息注入活跃回合：它自己的 Interaction 到此为 understood。
    state = conversationReducer(state, {
        type: 'frames',
        frames: [statusFrameOf(STEER, 'understood')],
        nowMs: 4_000,
    });

    assert.equal(working(state), true, '别的 Interaction 收口不许把这一轮判死');
    assert.notEqual(state.turns[0].status, 'completed', '工作状态行与中断入口据此常驻');
    assert.equal(pendingSteerCount(state.turns), 1, '排队读数仍然在');
    assert.equal(state.turns[0].activity, 'sensenova/x 生成中', '供应商阶段文案没有被清掉');
});

test('own_status_frame_settles_the_turn：属于本轮的终态帧照旧收口', () => {
    let state = queuedSteer(streamingTurn());
    state = conversationReducer(state, {
        type: 'frames',
        frames: [statusFrameOf(RUNNING, 'understood')],
        nowMs: 4_000,
    });

    assert.equal(state.turns[0].status, 'completed', '本轮自己的终态仍然收口');
    assert.equal(working(state), false, '收口之后不再宣称在跑');
    assert.equal(pendingSteerCount(state.turns), 0, '没有在跑的轮次就没有排队读数');
});

test('own_interrupted_status_frame_still_marks_the_turn：中断结论同样按归属落帧', () => {
    let state = streamingTurn();
    state = conversationReducer(state, {
        type: 'frames',
        frames: [statusFrameOf(RUNNING, 'interrupted')],
        nowMs: 4_000,
    });

    assert.equal(state.turns[0].status, 'interrupted');
});

test('unattributed_status_frame_keeps_the_previous_rule：认不出归属的帧仍按活跃轮次处理', () => {
    let state = streamingTurn();
    state = conversationReducer(state, {
        type: 'frames',
        frames: [
            {
                kind: 'frame',
                subscribeId: 'session-turns:01SESSION0000000000000000000',
                eventType: 'interaction.status',
                cursor: null,
                payloadJson: JSON.stringify('understood'),
            },
        ],
        nowMs: 4_000,
    });

    assert.equal(state.turns[0].status, 'completed', '没有归属标识时退回旧口径，不把收口弄丢');
});

test('subscribed_interaction_id_reads_only_interaction_subscriptions：订阅标识解析', () => {
    assert.equal(subscribedInteractionId(`interaction:${RUNNING}`), RUNNING);
    assert.equal(subscribedInteractionId('session-turns:01SESSION0000'), '');
    assert.equal(subscribedInteractionId(''), '');
});
