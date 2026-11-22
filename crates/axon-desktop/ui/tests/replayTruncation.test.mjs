import test from 'node:test';
import assert from 'node:assert/strict';

import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';
import {
    acceptsFrameSequence,
    replayResume,
    replayTruncationNotice,
} from '../src/conversation/replayTruncation.ts';

/*
 * 截断可见与按序号续读。
 *
 * 网关在订阅确认帧里给出「这次回放没追到 durable 尾」与真实续读游标；客户端必须两件
 * 都消费：把残缺说出来，并从服务端给的位置续读。把残缺当完整、或从 0 重放，都会做出
 * 一份看不出问题的假历史。
 */

const RESUME_CURSOR = JSON.stringify({ generation: 'g', sequence: 6230 });

/** 一条订阅确认帧。 */
function subscribed({ replayTruncated, resumeCursor }) {
    return {
        kind: 'subscribed',
        subscribeId: 'interaction:i1',
        replayTruncated,
        resumeCursor,
    };
}

test('replay_truncated_renders_partial_history_notice', () => {
    const state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'frames',
        frames: [subscribed({ replayTruncated: true, resumeCursor: RESUME_CURSOR })],
        nowMs: 9_000,
    });
    assert.equal(
        state.replayNotice,
        '历史仅显示到第 6230 帧',
        '残缺回放必须如实说出显示到第几帧，不能当完整历史渲染',
    );

    // 后续一次读到尾的确认帧把提示撤下：补齐之后不再说残缺。
    const cleared = conversationReducer(state, {
        type: 'frames',
        frames: [subscribed({ replayTruncated: false, resumeCursor: null })],
        nowMs: 9_500,
    });
    assert.equal(cleared.replayNotice, null);
    assert.equal(replayTruncationNotice({ replayTruncated: false, resumeCursor: null }), null);

    // 截断却没有游标：只说明残缺，不编帧号。
    assert.equal(
        replayTruncationNotice({ replayTruncated: true, resumeCursor: null }),
        '历史回放未追到最新，界面显示的不是完整历史',
    );
});

test('replay_truncated_triggers_cursor_resume', () => {
    const decision = replayResume({ replayTruncated: true, resumeCursor: RESUME_CURSOR });
    assert.deepEqual(
        decision,
        { resumeCursor: RESUME_CURSOR, sequence: 6230 },
        '续读必须用服务端给的游标，而不是从 0 或当作完整',
    );
    assert.equal(decision.sequence > 0, true);

    // 完整回放不需要续订。
    assert.equal(replayResume({ replayTruncated: false, resumeCursor: null }), null);
    // 截断却没有游标：没有可续的位置，不自编起点。
    assert.equal(replayResume({ replayTruncated: true, resumeCursor: null }), null);

    // 重叠帧按订阅内序号去重：不得长出重复气泡。
    assert.equal(acceptsFrameSequence(null, 6230), true);
    assert.equal(acceptsFrameSequence(6230, 6230), false, '同一帧重放时丢弃');
    assert.equal(acceptsFrameSequence(6230, 6229), false);
    assert.equal(acceptsFrameSequence(6230, 6231), true);
    assert.equal(acceptsFrameSequence(6230, null), true, '没有序号的瞬态帧照常放行');
});
