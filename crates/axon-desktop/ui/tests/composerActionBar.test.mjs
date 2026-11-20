/*
 * 输入坞底部操作行：主按钮的两用语义与模型选择器的锁定形态。
 *
 * 两条都是「这个控件该不该在、点下去会发生什么」的形态判断，与后端无关，因此用静态渲染
 * 逐条钉住。
 */

import React from 'react';
import test from 'node:test';
import assert from 'node:assert/strict';
import { renderToStaticMarkup } from 'react-dom/server';
import { ComposerActionBar } from '../src/components/conversation/ComposerActionBar.tsx';

function render(overrides = {}) {
  return renderToStaticMarkup(
    React.createElement(ComposerActionBar, {
      compact: false,
      enabled: true,
      readOnly: false,
      hintVisible: false,
      hintText: 'Shift + Enter 换行',
      model: 'glm-5',
      models: ['glm-5', 'glm-4.6'],
      effort: 'high',
      configBusy: false,
      configLocked: false,
      showStop: false,
      sendable: false,
      busy: false,
      commandRunning: false,
      hasContent: false,
      working: false,
      voiceAvailable: false,
      voiceActive: false,
      onToggleVoice: () => undefined,
      onPickAttachment: () => undefined,
      onModelChange: () => undefined,
      onReasoningEffortChange: () => undefined,
      onInterrupt: () => undefined,
      onSubmit: () => undefined,
      ...overrides,
    }),
  );
}

test('有内容时主按钮切回发送键，同一位置不再表达停止', () => {
  const html = render({ hasContent: true, sendable: true, working: true });
  assert.match(html, /aria-label="发送"/);
  assert.doesNotMatch(
    html,
    /aria-label="停止生成"/,
    '主按钮已经表达发送，不能再并列一个语义相反的停止键',
  );
});

test('输入框空着时主按钮就是停止键', () => {
  const html = render({ hasContent: false, working: true, showStop: true });
  const stops = html.match(/aria-label="停止生成"/g) ?? [];
  assert.equal(stops.length, 1);
  assert.doesNotMatch(html, /aria-label="发送"/);
});

test('本轮不可中断时没有停止入口', () => {
  const html = render({ hasContent: true, sendable: true });
  assert.doesNotMatch(html, /aria-label="停止生成"/);
});

test('非主会话仍显示模型，但选择器锁定不可变更', () => {
  const html = render({ readOnly: false, configLocked: true, enabled: true });
  assert.match(html, /模型与推理等级/, '模型区域必须照常显示当前模型');
  assert.match(html, /glm-5/);
  assert.match(html, /disabled/, '非主会话的模型与思考强度不可变更');
});

test('只读目标会话也显示模型（锁定），不整块隐藏', () => {
  const html = render({ readOnly: true, enabled: false, configLocked: true });
  assert.match(html, /模型与推理等级/);
  assert.match(html, /glm-5/);
  assert.match(html, /disabled/);
});
