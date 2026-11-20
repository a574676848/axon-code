/*
 * 工作状态行的挂载与文案口径。
 *
 * 这一行有两种在场理由：真的有一轮在跑（耗时与 ESC 中断），或者正文还在装载
 * （只写「正在加载会话…」）。装载期间一个请求都还没发出，因此不能承诺 ESC：
 * 写出来只会让人去停一件没在跑的事。
 */

import test from 'node:test';
import assert from 'node:assert/strict';
import { workingLineOf } from '../src/conversation/transcript/derive.ts';

/** 造一轮在跑的快照。 */
function turn(activity, elapsedMs) {
  return { activity, elapsedMs };
}

test('正文装载中只写加载句，且不承诺 ESC', () => {
  const line = workingLineOf(null, 0, true);
  assert.equal(line.primary, '正在加载会话…');
  assert.equal(line.interruptible, false, '装载期间 Esc 不接管中断');
});

test('装载中也照常给排队条数，不伪造耗时', () => {
  const line = workingLineOf(turn('sensenova/x 生成中', 3_000), 2, true);
  assert.equal(line.primary, '正在加载会话…');
  assert.equal(line.queued, 2);
});

test('装载完成后才进入供应商阶段并允许中断', () => {
  const line = workingLineOf(turn('sensenova/x 生成中', 3_000), 0, false);
  assert.equal(line.primary, 'sensenova/x 生成中');
  assert.equal(line.elapsedMs, 3_000);
  assert.equal(line.interruptible, true);
});

test('没有供应商事实时写中性句', () => {
  assert.equal(workingLineOf(null, 0, false).primary, '正在连接供应商…');
});
