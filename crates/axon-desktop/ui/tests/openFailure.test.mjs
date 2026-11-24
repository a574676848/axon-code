/*
 * 打开会话失败的判定。
 *
 * 一个字段两种用途（打开失败与提交失败共用 `submitError`），而两者的处置完全不同：
 * 提交失败时本地轮次已经立起来了，正文里有未送达标记；打开失败时一条正文都没有，
 * 屏幕上只剩空白。这里钉住「什么时候算后者」，以及「回执清空之后卡不跟着消失」。
 */

import test from 'node:test';
import assert from 'node:assert/strict';
import { nextOpenFailure } from '../src/conversation/openFailure.ts';

const base = { error: null, loadingHistory: false, opening: false, hasTurns: false };

test('读不回来且没有装载在途时认下这条失败', () => {
  assert.equal(nextOpenFailure(null, { ...base, error: '读取会话历史失败' }), '读取会话历史失败');
});

test('回执到点被清空，卡仍然留着', () => {
  // 控制器那条回执只活 6 秒；卡片自己记住结论，不能跟着一起消失。
  const remembered = nextOpenFailure(null, { ...base, error: '读取会话历史失败' });
  assert.equal(nextOpenFailure(remembered, { ...base, error: null }), '读取会话历史失败');
});

test('正文终于落地就撤下这张卡', () => {
  assert.equal(
    nextOpenFailure('读取会话历史失败', { ...base, hasTurns: true }),
    null,
  );
});

test('提交失败不算打开失败：本地轮次已经立起来了', () => {
  assert.equal(
    nextOpenFailure(null, { ...base, hasTurns: true, error: '发送失败：会话不存在' }),
    null,
  );
});

test('装载在途时保持上一份结论，不在中途一收一放', () => {
  assert.equal(
    nextOpenFailure('读取会话历史失败', { ...base, loadingHistory: true }),
    '读取会话历史失败',
  );
  assert.equal(
    nextOpenFailure('读取会话历史失败', { ...base, opening: true }),
    '读取会话历史失败',
  );
});

test('重连成功之后换一条结论：新失败取新的，没有失败就不亮卡', () => {
  assert.equal(nextOpenFailure('旧结论', { ...base, error: '连不上后台服务' }), '连不上后台服务');
});
