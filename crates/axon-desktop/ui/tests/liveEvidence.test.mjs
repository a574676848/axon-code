import test from 'node:test';
import assert from 'node:assert/strict';

import { liveEvidence } from '../src/conversation/liveEvidence.ts';

/*
 * 运行态证据过期时的降级表达。
 *
 * 「在跑」必须能指回带租约且未过期的行：界面自己的投影在进程消失、租约过期之后
 * 不会自己消失。证据一旦不再成立，界面不得继续显示「在跑」，而要改成「证据过期」
 * 并给出可执行建议。
 *
 * 反过来，刚提交的轮次还没有被服务端认领时，「没有在跑轮次」说的是「还没开始」，
 * 不是「已经没有执行者」；上一轮的空闲也不许算进这一轮的静默。这两条一起构成
 * 「过了很久再发一条消息，不该立刻报过期」。
 */

const NOW = 1_760_000_000_000;

/** 一条在跑的证据：租约还剩 12 秒。 */
function live(overrides = {}) {
    return {
        sessionId: 'session-1',
        running: true,
        leaseRemainingMs: 12_000,
        lastFrameSequence: 6230,
        idleForMs: 3_400,
        queuedTurns: 0,
        ...overrides,
    };
}

/** 一次证据读数；默认这一轮已经跑了 10 分钟，启动宽限早已过去。 */
function reading(overrides = {}) {
    return {
        live: live(),
        receivedAtMs: NOW,
        unreachableSinceMs: null,
        activeSinceMs: NOW - 600_000,
        ...overrides,
    };
}

test('desktop_shows_evidence_expired_instead_of_running', () => {
    // 证据仍然成立：照常显示在跑。
    const fresh = liveEvidence(true, reading(), NOW + 1_000);
    assert.equal(fresh.state, 'running');
    assert.equal(fresh.showRunning, true);
    assert.equal(fresh.title, null);

    // 租约到期之后：不得继续显示在跑，改为证据过期并给可执行建议。
    const expired = liveEvidence(true, reading(), NOW + 12_001);
    assert.equal(expired.state, 'expired');
    assert.equal(expired.showRunning, false, '证据过期时界面不再宣称在跑');
    assert.equal(expired.title, '运行证据已过期');
    assert.equal(expired.action, '重启后台服务', '给出可执行建议，不只是一句状态');
    assert.equal(expired.advice.includes('重启后台服务'), true);

    // 服务端直接说没有带租约的在跑轮次：同样降级。
    const noLease = liveEvidence(
        true,
        reading({ live: live({ running: false, leaseRemainingMs: null }) }),
        NOW + 1_000,
    );
    assert.equal(noLease.state, 'expired');
    assert.equal(noLease.showRunning, false);

    // 静默过久：即使租约还挂着也不再宣称在跑。
    const idle = liveEvidence(true, reading({ live: live({ idleForMs: 200_000 }) }), NOW + 1_000);
    assert.equal(idle.state, 'expired');
    assert.equal(idle.showRunning, false);

    // 后台服务已经读不到证据：连续失败超过阈值就降级，不必等租约自然到期。
    const unreachable = liveEvidence(
        true,
        reading({ unreachableSinceMs: NOW }),
        NOW + 21_000,
    );
    assert.equal(unreachable.state, 'expired');
    assert.equal(unreachable.showRunning, false);

    // 界面本来就没在跑：不出现任何卡片。
    const idleUi = liveEvidence(false, null, NOW);
    assert.equal(idleUi.state, 'unknown');
    assert.equal(idleUi.title, null);

    // 在跑但还没有任何证据：不改口，也不谎称过期。
    const unknown = liveEvidence(true, null, NOW);
    assert.equal(unknown.state, 'unknown');
    assert.equal(unknown.showRunning, true);
    assert.equal(unknown.title, null);
});

test('freshly_submitted_turn_is_not_reported_as_expired', () => {
    // 消息刚发出、服务端还没认领：running 为假是正常空档，界面不许据此判过期。
    const justSent = liveEvidence(
        true,
        reading({ live: live({ running: false, leaseRemainingMs: null }), activeSinceMs: NOW }),
        NOW + 1_000,
    );
    assert.equal(justSent.state, 'unknown', '提交到认领之间不是过期');
    assert.equal(justSent.showRunning, true);
    assert.equal(justSent.title, null);

    // 同一个空档里租约余量被本地流逝时间吃穿：也不判过期，这一轮还没开始。
    const leaseNotYet = liveEvidence(
        true,
        reading({ live: live({ leaseRemainingMs: 2_000 }), activeSinceMs: NOW }),
        NOW + 5_000,
    );
    assert.equal(leaseNotYet.state, 'unknown');
    assert.equal(leaseNotYet.showRunning, true);

    // 宽限过去仍然没有带租约的轮次：说明这一轮真的没有执行者，该报就报。
    const stuck = liveEvidence(
        true,
        reading({
            live: live({ running: false, leaseRemainingMs: null }),
            activeSinceMs: NOW - 11_000,
        }),
        NOW,
    );
    assert.equal(stuck.state, 'expired');
    assert.equal(stuck.title, '运行证据已过期');
});

test('queued_turn_is_starting_not_expired', () => {
    // 服务端如实报出排队轮次：宽限早已过去也不判过期，它只是还没被领取。
    const queued = liveEvidence(
        true,
        reading({
            live: live({ running: false, leaseRemainingMs: null, queuedTurns: 1 }),
            activeSinceMs: NOW - 600_000,
        }),
        NOW,
    );
    assert.equal(queued.state, 'unknown', '排队轮次不是过期');
    assert.equal(queued.showRunning, true);
    assert.equal(queued.title, null);

    // 排队轮次被领取之后，证据换成带租约的在跑轮次，照常显示在跑。
    const claimed = liveEvidence(
        true,
        reading({ live: live({ queuedTurns: 0 }), activeSinceMs: NOW - 600_000 }),
        NOW,
    );
    assert.equal(claimed.state, 'running');
});

test('session_silence_before_this_turn_is_not_this_turn_silence', () => {
    // 会话静默了两小时才发的消息：静默只算本轮，不许把上一轮的空闲当成本轮卡死。
    const afterLongIdle = liveEvidence(
        true,
        reading({ live: live({ idleForMs: 7_200_000 }), activeSinceMs: NOW }),
        NOW + 5_000,
    );
    assert.equal(afterLongIdle.state, 'running');
    assert.equal(afterLongIdle.showRunning, true, '首字还没到，但这一轮确实在跑');

    // 本轮自己静默超过阈值：仍然降级。
    const stuck = liveEvidence(
        true,
        reading({ live: live({ idleForMs: 200_000 }), activeSinceMs: NOW - 200_000 }),
        NOW,
    );
    assert.equal(stuck.state, 'expired');
    assert.equal(stuck.reason.includes('没有新事件'), true);
});
