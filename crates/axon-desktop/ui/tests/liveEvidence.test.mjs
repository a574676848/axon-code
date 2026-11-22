import test from 'node:test';
import assert from 'node:assert/strict';

import { liveEvidence } from '../src/conversation/liveEvidence.ts';

/*
 * 运行态证据过期时的降级表达。
 *
 * 「在跑」必须能指回带租约且未过期的行：界面自己的投影在进程消失、租约过期之后
 * 不会自己消失。证据一旦不再成立，界面不得继续显示「在跑」，而要改成「证据过期」
 * 并给出可执行建议。
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
        ...overrides,
    };
}

/** 一次证据读数。 */
function reading(overrides = {}) {
    return {
        live: live(),
        receivedAtMs: NOW,
        unreachableSinceMs: null,
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
