/*
 * 过渡面的取值表。
 *
 * 这张表是「切换会话时正文区看到什么」的全部决策，逐条钉住：打开过程中必须有东西
 * 占住正文区，此前没有正文可看时过渡面更不能塌成空白。
 */

import test from 'node:test';
import assert from 'node:assert/strict';
import { coverFace, showsHero } from '../src/conversation/stage.ts';

test('两屏正文之间切换：快到看不出来时过渡面不出场', () => {
    assert.equal(coverFace(true, false, false), 'none');
});

test('打开一条有内容的会话：慢过阈值就亮骨架，不留空白', () => {
    // 上一屏可能正是一块空画布（首屏刚点进一条会话）。此时返回 none 等于让正文区
    // 一路空白到回放落地，而一篇长会话要读好几段索引。
    assert.equal(coverFace(true, true, false), 'skeleton');
    // 快到看不出来的那一下仍由上一屏顶着，只是不额外盖东西。
    assert.equal(coverFace(true, false, false), 'none');
});

test('没有正文可看时过渡面留在原处，不塌成空白', () => {
    // 首屏，或读完了仍然一条正文都没有：这是「本来就没有内容」的形态。
    assert.equal(coverFace(true, false, true), 'hero');
    assert.equal(coverFace(true, true, true), 'hero');
});

test('打开窗口之外：只认「有没有正文可看」', () => {
    assert.equal(coverFace(false, false, true), 'hero');
    assert.equal(coverFace(false, true, true), 'hero');
    assert.equal(coverFace(false, false, false), 'none');
    // 慢过阈值的痕迹不带进下一次打开。
    assert.equal(coverFace(false, true, false), 'none');
});

/** 首屏判定的输入：除声明项外都给「已就绪、没有正文、不是新建」的常态。 */
function hero(overrides = {}) {
    return showsHero({
        noSelection: false,
        pending: false,
        turns: 0,
        opening: false,
        loadingHistory: false,
        ready: true,
        ...overrides,
    });
}

test('首屏判定：没有选中会话时就是首屏', () => {
    assert.equal(hero({ noSelection: true }), true);
});

test('首屏判定：有正文就不画首屏', () => {
    assert.equal(hero({ turns: 2 }), false);
    assert.equal(hero({ turns: 2, pending: true }), false);
});

test('首屏判定：刚建好、还没有内容的会话仍是首屏', () => {
    assert.equal(hero({ pending: true, ready: false }), true);
});

test('首屏判定：读完了却一条正文都没有，回落首屏而不是空白', () => {
    assert.equal(hero({ ready: true }), true);
});

test('首屏判定：装载在途时不抢过渡面的位置', () => {
    assert.equal(hero({ opening: true }), false);
    assert.equal(hero({ loadingHistory: true }), false);
    assert.equal(hero({ ready: false }), false);
});
