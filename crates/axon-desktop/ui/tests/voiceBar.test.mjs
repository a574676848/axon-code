/*
 * 坞内语音态带：四态可辨、同一时刻只有一个焦点，复用实测声波。
 * 用静态渲染逐条钉住「每个相位长什么样」，不碰计时器与后端。
 */

import React from 'react';
import test from 'node:test';
import assert from 'node:assert/strict';
import { renderToStaticMarkup } from 'react-dom/server';
import { ComposerVoiceBar } from '../src/components/conversation/ComposerVoiceLayer.tsx';

function voice(overrides = {}) {
  return {
    available: true,
    phase: 'listening',
    partial: '',
    error: null,
    active: true,
    elapsedMs: 0,
    holdProgress: 0,
    permission: null,
    toggle: () => undefined,
    finish: () => undefined,
    cancel: () => undefined,
    confirmPermission: () => undefined,
    dismissPermission: () => undefined,
    openSettings: () => undefined,
    retryPermission: () => undefined,
    ...overrides,
  };
}

function render(overrides) {
  return renderToStaticMarkup(React.createElement(ComposerVoiceBar, { voice: voice(overrides) }));
}

test('待机不渲染，坞形不变', () => {
  assert.equal(render({ phase: 'idle' }), '');
});

test('四态各有专属标记，远看分得清', () => {
  const labels = {
    arming: '准备中',
    listening: '聆听中',
    thinking: '整理中',
    speaking: '朗读中',
  };
  for (const [phase, label] of Object.entries(labels)) {
    const html = render({ phase });
    assert.match(html, new RegExp(`data-voice-status="${phase}"`), `${phase} 必须带自己的状态标记`);
    assert.ok(html.includes(label), `${phase} 必须显示自己的焦点词`);
  }
});

test('聆听与朗读复用实测声波标识，准备与整理用转圈', () => {
  assert.match(render({ phase: 'listening' }), /<canvas/);
  assert.match(render({ phase: 'speaking' }), /<canvas/);
  assert.doesNotMatch(render({ phase: 'thinking' }), /<canvas/);
});

test('实时转写出现时成为聆听态的唯一焦点', () => {
  const html = render({ phase: 'listening', partial: '帮我查一下订单' });
  assert.ok(html.includes('帮我查一下订单'));
  assert.doesNotMatch(html, /聆听中/, '有转写时不再并列状态词');
});

test('冷启动超时把焦点让给连接预期', () => {
  const html = render({ phase: 'arming', elapsedMs: 4000 });
  assert.ok(html.includes('正在连接语音提供方'));
  assert.doesNotMatch(html, /准备中/);
});

test('态带不再堆叠计时读数', () => {
  const html = render({ phase: 'listening', elapsedMs: 65000 });
  assert.doesNotMatch(html, /tabular-nums/, '计时不进态带');
});
