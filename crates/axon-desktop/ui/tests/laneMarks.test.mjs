import test from 'node:test';
import assert from 'node:assert/strict';
import { clearedAtSettle, markForNotice } from '../src/conversation/laneMarks.ts';

test('待答标识在前台也要挂上，因为它是「有事等你做」', () => {
  assert.deepEqual(
    markForNotice({ status: 'question_pending', foreground: true }, 'lane-a'),
    { laneId: 'lane-a', status: 'question_pending' },
  );
  assert.deepEqual(
    markForNotice({ status: 'question_suspended', foreground: true }, 'lane-a'),
    { laneId: 'lane-a', status: 'question_suspended' },
  );
});

test('前台的新结果不挂标识，托盘里的任何终态都挂', () => {
  // 用户正看着这条会话：结果已经在屏幕上，再挂一次标识只是噪音。
  assert.equal(markForNotice({ status: 'completed', foreground: true }, 'lane-a'), null);
  assert.equal(markForNotice({ status: 'failed', foreground: true }, 'lane-a'), null);

  assert.deepEqual(
    markForNotice({ status: 'completed', foreground: false }, 'lane-a'),
    { laneId: 'lane-a', status: 'completed' },
  );
  assert.deepEqual(
    markForNotice({ status: 'interrupted', foreground: false }, 'lane-a'),
    { laneId: 'lane-a', status: 'interrupted' },
  );
});

test('没有选中会话时任何推送都不挂标识', () => {
  assert.equal(markForNotice({ status: 'question_pending', foreground: false }, null), null);
  assert.equal(markForNotice({ status: 'completed', foreground: false }, null), null);
});

test('待答标识只随提问结算撤下，结算前与未知时一律保留', () => {
  const marks = { 'lane-a': 'question_pending', 'lane-b': 'completed' };

  // 还在等回答：不撤。
  assert.equal(clearedAtSettle(marks, 'lane-a', true), null);
  // 结算了：撤。
  assert.equal(clearedAtSettle(marks, 'lane-a', false), 'lane-a');
  // 新结果标识不归这条规则管，它随开关会话消失。
  assert.equal(clearedAtSettle(marks, 'lane-b', false), null);
  // 没有标识、没有车道：都无需动作。
  assert.equal(clearedAtSettle({}, 'lane-a', false), null);
  assert.equal(clearedAtSettle(marks, null, false), null);
});
