import test from 'node:test';
import assert from 'node:assert/strict';
import { newestReceivedAtMs, turnsNewerThanFloor } from '../src/conversation/pagingBounds.ts';

/*
 * 增量接管的时间下界。
 *
 * 场景是真实发生过的：向前翻过页之后，最新一页窗口里仍带着已被替换成历史的老轮，而增量接口
 * 只按「本视图还不认识」去重，于是这些老轮会被追加到对话末尾，看起来像是刚刚发生的一轮。
 * 这里固定判据：只有不早于已加载最新时刻的轮次才算新轮，同刻的照常收下（毫秒精度下同刻有两条
 * 是可能的），首屏还没有下界时全收。
 */

const turn = (interactionId, receivedAtMs) => ({ interactionId, receivedAtMs });
const idOf = (entry) => entry.interactionId;
const known = (...ids) => new Set(ids);

test('补回的老轮早于已加载最新时刻，不当成新轮', () => {
    const page = [turn('old-1', 3_000), turn('old-2', 4_000)];
    const fresh = turnsNewerThanFloor(page, known('live-1'), idOf, 5_000);
    assert.deepEqual(fresh, []);
});

test('同刻轮次照常收下：接收时刻只到毫秒，不能因为相等而漏掉', () => {
    const page = [turn('same-ms', 5_000), turn('newer', 6_000)];
    const fresh = turnsNewerThanFloor(page, known(), idOf, 5_000);
    assert.deepEqual(
        fresh.map((entry) => entry.interactionId),
        ['same-ms', 'newer'],
    );
});

test('首屏没有下界时，除已认识的那几条外全收', () => {
    const page = [turn('a', 1_000), turn('b', 2_000)];
    const fresh = turnsNewerThanFloor(page, known('a'), idOf, null);
    assert.deepEqual(
        fresh.map((entry) => entry.interactionId),
        ['b'],
    );
});

test('一页里最新的接收时刻取最大值；空页没有下界', () => {
    assert.equal(newestReceivedAtMs([]), null);
    assert.equal(newestReceivedAtMs([turn('a', 1_000), turn('b', 9_000), turn('c', 4_000)]), 9_000);
});
