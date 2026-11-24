import test from 'node:test';
import assert from 'node:assert/strict';
import { clearedAtSettle, markForNotice } from '../src/conversation/sessionMarks.ts';

test('待答标识在前台也要挂上，因为它是「有事等你做」', () => {
  assert.deepEqual(
    markForNotice({ status: 'question_pending', foreground: true, sessionId: 'session-a' }, 'session-a'),
    { sessionId: 'session-a', status: 'question_pending' },
  );
  assert.deepEqual(
    markForNotice({ status: 'question_suspended', foreground: true, sessionId: 'session-a' }, 'session-a'),
    { sessionId: 'session-a', status: 'question_suspended' },
  );
});

test('前台的新结果同样挂标识：系统通知已经让位给这颗状态点', () => {
  // 前台不再发系统通知，标识是唯一的提示，因此两个分支的结论必须一致。
  for (const foreground of [true, false]) {
    for (const [status, expected] of [
      ['completed', 'done'],
      ['failed', 'error'],
      ['interrupted', 'warning'],
    ]) {
      const mark = markForNotice({ status, foreground, sessionId: 'session-a' }, 'session-a');
      assert.deepEqual(mark, { sessionId: 'session-a', status });
      assert.equal(typeof expected, 'string');
    }
  }
});

test('标识落在事件自己带来的会话上，没带时才回落到当前选中', () => {
  // 用户已经切走：事件仍须挂到它真正所属的那一行，不能跟着光标跑。
  assert.deepEqual(
    markForNotice({ status: 'completed', foreground: true, sessionId: 'session-b' }, 'session-a'),
    { sessionId: 'session-b', status: 'completed' },
  );
  // 旧宿主不带会话标识时按旧口径回落到当前会话。
  assert.deepEqual(
    markForNotice({ status: 'completed', foreground: true }, 'session-a'),
    { sessionId: 'session-a', status: 'completed' },
  );
  // 两处都没有会话：无从落位，不挂。
  assert.equal(markForNotice({ status: 'completed', foreground: true }, null), null);
  assert.equal(markForNotice({ status: 'completed', foreground: true, sessionId: '  ' }, null), null);
});

test('待答标识只随提问结算撤下，结算前与未知时一律保留', () => {
  const marks = { 'session-a': 'question_pending', 'session-b': 'completed' };

  // 还在等回答：不撤。
  assert.equal(clearedAtSettle(marks, 'session-a', true), null);
  // 结算了：撤。
  assert.equal(clearedAtSettle(marks, 'session-a', false), 'session-a');
  // 新结果标识不归这条规则管，它随选中该会话消失。
  assert.equal(clearedAtSettle(marks, 'session-b', false), null);
  // 没有标识、没有车道：都无需动作。
  assert.equal(clearedAtSettle({}, 'session-a', false), null);
  assert.equal(clearedAtSettle(marks, null, false), null);
});
