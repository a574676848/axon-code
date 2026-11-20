/*
 * 输入坞底部操作行：停止入口常驻与只读会话的模型选择器。
 *
 * 两条都是「这个控件该不该在」的形态判断，与后端无关，因此用静态渲染逐条钉住。
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
      stopAvailable: false,
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

test('输入框有内容时，停止图标与发送键并排都在', () => {
  const html = render({ hasContent: true, sendable: true, working: true, stopAvailable: true });
  assert.match(html, /aria-label="停止生成"/, '有内容时停止入口不能消失');
  assert.match(html, /aria-label="发送"/);
});

test('输入框空着时主按钮就是停止键，不再多画一枚', () => {
  const html = render({ hasContent: false, working: true, stopAvailable: true, showStop: true });
  const stops = html.match(/aria-label="停止生成"/g) ?? [];
  assert.equal(stops.length, 1);
  assert.doesNotMatch(html, /aria-label="发送"/);
});

test('本轮不可中断时没有停止入口', () => {
  const html = render({ hasContent: true, sendable: true });
  assert.doesNotMatch(html, /aria-label="停止生成"/);
});

test('只读会话整块不渲染模型选择器', () => {
  const html = render({ readOnly: true, enabled: false, model: 'glm-5' });
  assert.doesNotMatch(html, /模型与推理等级/);
  assert.doesNotMatch(html, /glm-5/);
});

test('可写会话照常给出模型选择器', () => {
  const html = render();
  assert.match(html, /模型与推理等级/);
  assert.match(html, /glm-5/);
});
