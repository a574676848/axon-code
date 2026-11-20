/*
 * 消息导航条的稳定签名。
 *
 * 正文每来一批 token 都会换一次轮次数组，观察者若跟着数组引用重建，一屏几十个
 * 轮次的 IntersectionObserver 就会反复注册注销。签名只认「哪些轮次带用户那一句」，
 * 因此同一批轮次继续变长时签名不变。
 */

import test from 'node:test';
import assert from 'node:assert/strict';
import { navigableSignature } from '../src/components/conversation/MessageRail.tsx';

/** 造一轮：`text` 为 null 表示这一轮没有用户那一句。 */
function turn(id, text) {
  return {
    id,
    cells: text === null ? [{ role: 'assistant', text: '回答', id: `${id}:a` }] : [{ role: 'user', text, id: `${id}:u` }],
  };
}

test('同一批轮次的正文变长时签名不变', () => {
  const before = [turn('t1', '第一问'), turn('t2', '第二问')];
  const after = [
    { id: 't1', cells: [{ role: 'user', text: '第一问', id: 't1:u' }, { role: 'assistant', text: '很长的回答', id: 't1:a' }] },
    turn('t2', '第二问'),
  ];
  assert.equal(navigableSignature(before), navigableSignature(after));
  assert.equal(navigableSignature(before), 't1|t2');
});

test('没有用户那一句的轮次不进签名', () => {
  assert.equal(navigableSignature([turn('t1', '问'), turn('t2', null)]), 't1');
  assert.equal(navigableSignature([turn('t1', null)]), '');
});

test('真的多一轮或少一轮时签名才变', () => {
  assert.notEqual(
    navigableSignature([turn('t1', '问')]),
    navigableSignature([turn('t1', '问'), turn('t2', '再问')]),
  );
  assert.notEqual(
    navigableSignature([turn('t1', '问'), turn('t2', '再问')]),
    navigableSignature([turn('t2', '再问')]),
  );
});

test('空白气泡不算导航目标', () => {
  assert.equal(navigableSignature([turn('t1', '   ')]), '');
});
