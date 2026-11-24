/*
 * 时间线上的计划单元。
 *
 * 它此前整块返回 null：过程里「更新过计划」这件事在正文中彻底不可见，而设计系统
 * 写的是「某一次 update_plan 的原子记录，默认收起成一行计数摘要」。这里钉住两件事：
 * 计数摘要的口径，以及默认收起时步骤明细不上屏。
 */

import React from 'react';
import test from 'node:test';
import assert from 'node:assert/strict';
import { renderToStaticMarkup } from 'react-dom/server';
import { PlanCard, planSummary } from '../src/components/conversation/PlanCard.tsx';

const STEPS = [
  { step: '读设计系统', status: 'completed' },
  { step: '改输入坞', status: 'running' },
  { step: '补测试', status: 'pending' },
];

test('计数摘要只列非零项，用中点连接', () => {
  assert.equal(planSummary(STEPS), '1 已完成 · 1 进行中 · 1 待处理');
  assert.equal(planSummary([{ step: 'a', status: 'completed' }]), '1 已完成');
  assert.equal(planSummary([]), '');
});

test('全是待处理时摘要照样成立', () => {
  assert.equal(
    planSummary([
      { step: 'a', status: 'pending' },
      { step: 'b', status: 'pending' },
    ]),
    '2 待处理',
  );
});

test('默认收起：只留一行计数摘要，步骤明细不上屏', () => {
  const html = renderToStaticMarkup(
    React.createElement(PlanCard, { title: '计划整理', steps: STEPS, defaultOpen: false }),
  );
  assert.match(html, /计划整理/);
  assert.match(html, /1 已完成/);
  assert.doesNotMatch(html, /读设计系统/, '收起时不该把步骤明细画出来');
});

test('展开后才出现步骤明细', () => {
  const html = renderToStaticMarkup(
    React.createElement(PlanCard, { title: '计划整理', steps: STEPS, defaultOpen: true }),
  );
  assert.match(html, /读设计系统/);
  assert.match(html, /补测试/);
});
