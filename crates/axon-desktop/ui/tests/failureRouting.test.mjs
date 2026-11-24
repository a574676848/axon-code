import test from 'node:test';
import assert from 'node:assert/strict';
import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';

/*
 * 失败路由的回归：三种入口都必须落成同一张失败卡。
 *
 * 真机上曾经出现两处走样：`turn_completed(status=failed)` 把后端的失败结论当正文渲染，
 * 于是一整段 JSON 直接铺在对话里；运行时失败事件则只落一条没有归因的提示。
 */

const NOW = 1_700_000_000_000;

function openTurn() {
    return conversationReducer(INITIAL_CONVERSATION, { type: 'open-turn', turnId: 't1', nowMs: NOW });
}

function frames(state, frames) {
    return conversationReducer(state, { type: 'frames', nowMs: NOW + 1_000, frames });
}

function failureCells(state) {
    return state.turns.flatMap((turn) => turn.cells).filter((cell) => cell.role === 'failure');
}

test('失败终态渲染成失败卡而不是把回执铺成正文', () => {
    const payload = {
        response:
            '本轮执行未完成：{"code":"UNKNOWN_FIELD","message":"未知请求字段：reasoning_summary","data":{"field":"reasoning_summary"},"traceId":"trace_1"}；reason=provider_streaming_runner:provider_error（runtime_failed）',
        status: 'failed',
    };
    const state = frames(openTurn(), [
        {
            kind: 'frame',
            subscribeId: 'interaction:i1',
            eventType: 'turn_completed',
            payloadJson: JSON.stringify(payload),
        },
    ]);

    const cards = failureCells(state);
    assert.equal(cards.length, 1);
    assert.equal(cards[0].report.code, 'UNKNOWN_FIELD');
    assert.equal(cards[0].report.reason, '未知请求字段：reasoning_summary');
    assert.equal(cards[0].text.length > 0, true, '原始回执保留给展开查看');

    const assistant = state.turns
        .flatMap((turn) => turn.cells)
        .filter((cell) => cell.role === 'assistant');
    assert.deepEqual(assistant, [], '失败结论不再作为助手正文出现');
});

test('终态载荷里的结构化失败优先于正文解读', () => {
    const payload = {
        response: '本轮执行未完成：上游返回 503',
        status: 'failed',
        failure: {
            category: 'provider_transport',
            code: 'upstream_503',
            redactedMessage: '上游返回 503',
            httpStatus: 503,
            retryable: true,
        },
    };
    const state = frames(openTurn(), [
        {
            kind: 'frame',
            subscribeId: 'interaction:i1',
            eventType: 'turn_completed',
            payloadJson: JSON.stringify(payload),
        },
    ]);

    const cards = failureCells(state);
    assert.equal(cards.length, 1);
    assert.equal(cards[0].report.code, 'upstream_503');
    assert.equal(cards[0].report.httpStatus, 503);
    assert.equal(cards[0].report.retryable, true);
});

test('运行时失败事件同样落成失败卡', () => {
    const state = frames(openTurn(), [
        {
            kind: 'frame',
            subscribeId: 'interaction:i1',
            eventType: 'wire_event',
            payloadJson: JSON.stringify({
                eventId: 'evt-1',
                kind: {
                    type: 'runtimeError',
                    payload: {
                        category: 'provider_transport',
                        code: 'invalid_parameter',
                        redactedMessage: 'Image too small (area=64px)',
                        httpStatus: 400,
                        retryable: true,
                        timestamp: 1,
                    },
                },
            }),
        },
    ]);

    const cards = failureCells(state);
    assert.equal(cards.length >= 1, true, '运行时失败必须有卡片');
    const report = cards[cards.length - 1].report;
    assert.ok(report !== null);
    assert.equal(report.code, 'invalid_parameter');
    assert.equal(report.httpStatus, 400);
});

test('解读不出结构的失败仍然原样显示，不丢信息', () => {
    const state = frames(openTurn(), [
        {
            kind: 'frame',
            subscribeId: 'interaction:i1',
            eventType: 'turn_completed',
            payloadJson: JSON.stringify({ response: '本轮执行未完成：网络不可达', status: 'failed' }),
        },
    ]);

    assert.equal(failureCells(state).length, 0);
    const assistant = state.turns
        .flatMap((turn) => turn.cells)
        .filter((cell) => cell.role === 'assistant')
        .map((cell) => cell.text);
    assert.equal(assistant.includes('本轮执行未完成：网络不可达'), true);
});
