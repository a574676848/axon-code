/*
 * Space 长按进度线：贴在麦克风入口上的状态标记与填充比例。
 * 用静态渲染逐条钉住「进度映射到什么形态」，与计时器、与后端都无关。
 */

import React from 'react';
import test from 'node:test';
import assert from 'node:assert/strict';
import { renderToStaticMarkup } from 'react-dom/server';
import { VoiceHoldProgress } from '../src/components/conversation/VoiceHoldProgress.tsx';

function render(progress) {
  return renderToStaticMarkup(React.createElement(VoiceHoldProgress, { progress }));
}

test('未长按时进度线是闲态且不填充', () => {
  const html = render(0);
  assert.match(html, /data-voice-hold-progress="idle"/);
  assert.match(html, /scaleX\(0\)/);
  assert.match(html, /opacity-0/);
});

test('按住时进度线进入 hold 并按比例填充', () => {
  const html = render(0.5);
  assert.match(html, /data-voice-hold-progress="hold"/);
  assert.match(html, /scaleX\(0\.5\)/);
  assert.match(html, /opacity-100/);
});

test('填满即转满即开，比例钳在 1', () => {
  const html = render(1.4);
  assert.match(html, /scaleX\(1\)/);
});

test('非法进度当作零，不画出乱值', () => {
  const html = render(Number.NaN);
  assert.match(html, /data-voice-hold-progress="idle"/);
  assert.match(html, /scaleX\(0\)/);
});
