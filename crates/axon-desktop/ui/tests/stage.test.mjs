import test from 'node:test';
import assert from 'node:assert/strict';
import { coverFace } from '../src/conversation/stage.ts';

/*
 * 过渡面的取值表。
 *
 * 这张表是「切换会话时正文区看到什么」的全部决策，逐条钉住：
 * 两屏正文之间切换要尽量不动（上一屏留到正文就绪），慢过阈值才亮骨架；
 * 此前没有正文可看时过渡面留在原处，不能塌成空白。
 */
test('两屏正文之间切换：快到看不出来时不盖任何东西', () => {
    assert.equal(coverFace(true, true, false, false), 'none');
});

test('两屏正文之间切换：慢过阈值才亮骨架', () => {
    assert.equal(coverFace(true, true, true, false), 'skeleton');
});

test('此前没有正文时过渡面留在原处，不塌成空白', () => {
    // 从首屏点进一条会话：上一屏是首屏，骨架不该接管它。
    assert.equal(coverFace(true, false, false, true), 'hero');
    assert.equal(coverFace(true, false, true, true), 'hero');
});

test('打开窗口之外：没有正文可看才是首屏', () => {
    assert.equal(coverFace(false, true, false, true), 'hero');
    assert.equal(coverFace(false, true, true, true), 'hero');
    assert.equal(coverFace(false, true, false, false), 'none');
    // 慢过阈值的痕迹不带进下一次打开：窗口一关就只认「有没有正文可看」。
    assert.equal(coverFace(false, true, true, false), 'none');
});
