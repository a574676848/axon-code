import test from 'node:test';
import assert from 'node:assert/strict';
import {
  COUNTDOWN_DANGER_THRESHOLD_MS,
  COUNTDOWN_WARN_THRESHOLD_MS,
  getCountdownPhase,
  shouldReattachLane,
  stripQuestionReceiptBlock,
  extractQuestionReceipt,
} from '../src/conversation/questions.ts';
import { conversationReducer } from '../src/conversation/transcript/reducer.ts';
import { INITIAL_CONVERSATION } from '../src/conversation/transcript/state.ts';

test('倒计时分级阶段判定：临界值切换准确无抖动', () => {
  assert.equal(getCountdownPhase(30001), 'normal');
  assert.equal(getCountdownPhase(30000), 'warning');
  assert.equal(getCountdownPhase(15000), 'warning');
  assert.equal(getCountdownPhase(10000), 'danger');
  assert.equal(getCountdownPhase(5000), 'danger');
  assert.equal(getCountdownPhase(0), 'danger');
});

test('问答回执结构化块剥离与双向解析', () => {
  const content = `1. 是否同意合并代码？ 回答：同意

[问答回执]
回答待决问题：call-deploy-42
回答内容：[{"id":"q1","selected":["同意"]}]`;

  // 纯可见正文剥离
  const stripped = stripQuestionReceiptBlock(content);
  assert.equal(stripped, '1. 是否同意合并代码？ 回答：同意');
  assert.ok(!stripped.includes('[问答回执]'));

  // 结构化回执提取
  const extracted = extractQuestionReceipt(content);
  assert.ok(extracted !== null);
  assert.equal(extracted.callId, 'call-deploy-42');
  assert.equal(extracted.answersJson, '[{"id":"q1","selected":["同意"]}]');

  // 无回执文本测试
  assert.equal(stripQuestionReceiptBlock('普通消息'), '普通消息');
  assert.equal(extractQuestionReceipt('普通消息'), null);
});

test('归约器在 state 级维护 pendingQuestions 映射，历史裁切后仍保留待决状态', () => {
  let state = INITIAL_CONVERSATION;

  // 1. 模拟收到 userQuestion 帧
  const questionFrame = {
    kind: 'event',
    eventType: 'wire_event',
    payloadJson: JSON.stringify({
      schemaVersion: 'axon.wire.v3',
      eventId: 'evt-1',
      correlationId: 'corr-1',
      session: { sessionId: 'lane-1' },
      sequence: 1,
      recordedAtMs: 1000,
      kind: {
        type: 'userQuestion',
        payload: {
          partId: 'part:q1',
          callId: 'call-101',
          status: 'pending',
          questions: [{ id: 'q1', question: '生产环境发布？' }],
          deadlineAtMs: 31000,
        },
      },
    }),
  };

  state = conversationReducer(state, {
    type: 'frames',
    frames: [questionFrame],
    nowMs: 1000,
  });

  // 验证 pendingQuestions 映射增量写入
  assert.ok(state.pendingQuestions['call-101']);
  assert.equal(state.pendingQuestions['call-101'].status, 'pending');
  assert.equal(state.pendingQuestions['call-101'].questions[0].question, '生产环境发布？');

  // 2. 模拟历史窗口截断（turns 被重置或裁切），验证 state.pendingQuestions 依然持有权威待决状态
  const trimmedTurnsState = {
    ...state,
    turns: [],
  };
  assert.ok(trimmedTurnsState.pendingQuestions['call-101']);
  assert.equal(trimmedTurnsState.pendingQuestions['call-101'].status, 'pending');

  // 3. 对账动作：服务端仍列为待决时按它的状态覆盖本地投影
  const reconciledState = conversationReducer(trimmedTurnsState, {
    type: 'reconcile-questions',
    questions: [
      {
        partId: 'part:q1',
        callId: 'call-101',
        status: 'suspended',
        questions: [{ id: 'q1', question: '生产环境发布？' }],
        deadlineAtMs: 31000,
      },
    ],
  });
  assert.equal(reconciledState.pendingQuestions['call-101'].status, 'suspended');

  // 4. 结算即离场：待答位只关心还需要用户回答的那几条
  const settledState = conversationReducer(reconciledState, {
    type: 'reconcile-questions',
    questions: [
      {
        partId: 'part:q1',
        callId: 'call-101',
        status: 'settled',
        questions: [{ id: 'q1', question: '生产环境发布？' }],
        deadlineAtMs: 31000,
      },
    ],
  });
  assert.equal(settledState.pendingQuestions['call-101'], undefined);

  // 5. 权威对账：服务端不再列出的待决项按已结算收敛，同样不再占据待答位
  const converged = conversationReducer(reconciledState, {
    type: 'reconcile-questions',
    questions: [],
    authoritative: true,
  });
  assert.equal(converged.pendingQuestions['call-101'], undefined);
});

test('走消息通道的作答之后必须重贴车道，回灌给等待者的那一档不必', () => {
  // 答案变成车道上的一条新用户消息，续跑的那一轮由守护进程开启：界面必须重贴。
  assert.equal(shouldReattachLane('user_message'), true);
  // 回灌给活等待者：界面手上那条订阅还在看着同一轮，后续帧自己会到。
  assert.equal(shouldReattachLane('tool_return'), false);
  // 网关没有下发投递方式时按需要重贴处理：停在原地比多贴一次更糟。
  assert.equal(shouldReattachLane(null), true);
  assert.equal(shouldReattachLane(undefined), true);
});

test('对账先于回放到达时，重放出来的挂起帧不得把答过的题摆回待答态', () => {
  /** 一条挂起帧：进入车道时历史回放会把它重放出来。 */
  const suspendedFrame = {
    kind: 'event',
    eventType: 'wire_event',
    payloadJson: JSON.stringify({
      schemaVersion: 'axon.wire.v3',
      eventId: 'evt-q-late',
      correlationId: 'corr-q-late',
      session: { sessionId: 'lane-1' },
      sequence: 3,
      recordedAtMs: 1_500,
      kind: {
        type: 'userQuestion',
        payload: {
          partId: 'part:q-late',
          callId: 'call-late',
          status: 'suspended',
          questions: [{ id: 'q1', question: '走哪个分支？' }],
          handoff: '我在等长官选定分支。',
          deadlineAtMs: 31_000,
        },
      },
    }),
  };

  /*
   * 进入车道时对账与回放是两条异步路径：对账拉的是权威记录（这一条其实已经被
   * 迟到作答结算），而时间线上的单元要等回放才被物化。对账先到、单元后到时，
   * 挂起帧会照着历史把单元重新画成「已挂起」，并给出注定被服务端拒绝的回入口。
   */
  const reconciled = conversationReducer(INITIAL_CONVERSATION, {
    type: 'reconcile-questions',
    authoritative: true,
    questions: [
      {
        partId: 'part:q-late',
        callId: 'call-late',
        status: 'settled',
        questions: [{ id: 'q1', question: '走哪个分支？' }],
        answers: [{ id: 'q1', selected: ['release'] }],
        handoff: '我在等长官选定分支。',
        deadlineAtMs: 31_000,
      },
    ],
  });
  const replayed = conversationReducer(reconciled, {
    type: 'frames',
    frames: [suspendedFrame],
    nowMs: 1_500,
  });

  const cells = replayed.turns.flatMap((turn) => turn.cells).filter((cell) => cell.role === 'question');
  assert.equal(cells.length, 1, '回放必须物化出这一个问答单元');
  assert.equal(cells[0].status, 'settled', '答过的题不得被重放的历史帧改回挂起态');
  assert.deepEqual(cells[0].answers, [{ id: 'q1', selected: ['release'] }], '作答内容按对账结论回填');
  assert.equal(
    replayed.pendingQuestions['call-late'].status,
    'settled',
    '这一条不得重新出现在待答位上（待答位只认还在等回答的那几档）',
  );
});

test('对账把权威状态同步到时间线单元：已结算的提问不再给出回入口', () => {
  /** 一条打开轮次的用户消息帧，让后续问答单元有归属的轮次。 */
  const userFrame = (sequence) => ({
    kind: 'event',
    eventType: 'wire_event',
    payloadJson: JSON.stringify({
      schemaVersion: 'axon.wire.v3',
      eventId: `evt-user-${sequence}`,
      correlationId: `corr-user-${sequence}`,
      session: { sessionId: 'lane-1' },
      sequence,
      recordedAtMs: 1000 + sequence,
      kind: { type: 'userMessage', payload: { partId: 'part:user', content: '开始' } },
    }),
  });
  /** 挂起状态的问答帧：单元在提问那一刻被物化进轮次。 */
  const questionFrame = (sequence) => ({
    kind: 'event',
    eventType: 'wire_event',
    payloadJson: JSON.stringify({
      schemaVersion: 'axon.wire.v3',
      eventId: `evt-q-${sequence}`,
      correlationId: 'corr-q',
      session: { sessionId: 'lane-1' },
      sequence,
      recordedAtMs: 1000 + sequence,
      kind: {
        type: 'userQuestion',
        payload: {
          partId: 'part:q9',
          callId: 'call-909',
          status: 'suspended',
          questions: [{ id: 'q1', question: '走哪个分支？' }],
          handoff: '我在等长官选定分支。',
          deadlineAtMs: 31000,
        },
      },
    }),
  });

  const state = conversationReducer(INITIAL_CONVERSATION, {
    type: 'frames',
    frames: [userFrame(1), questionFrame(2)],
    nowMs: 1002,
  });
  const questionCells = (value) =>
    value.turns.flatMap((turn) => turn.cells).filter((cell) => cell.role === 'question');
  assert.equal(questionCells(state).length, 1, '问答单元应已物化进轮次');
  assert.equal(questionCells(state)[0].status, 'suspended');
  assert.equal(state.pendingQuestions['call-909'].status, 'suspended');

  /*
   * 迟到作答发生在回合之外：它只发布结算事件，没有可供单元刷新的帧，因此从历史
   * 重建出来的单元会停在挂起态。对账必须同时改写单元，否则界面会一直显示旧状态，
   * 并给出注定被服务端拒绝的「重新打开」。
   */
  const authoritativeRecord = {
    partId: 'part:q9',
    callId: 'call-909',
    status: 'settled',
    questions: [{ id: 'q1', question: '走哪个分支？' }],
    answers: [{ id: 'q1', selected: ['release'] }],
    handoff: '我在等长官选定分支。',
    deadlineAtMs: 31000,
  };
  const reconciled = conversationReducer(state, {
    type: 'reconcile-questions',
    authoritative: true,
    questions: [authoritativeRecord],
  });

  const cells = questionCells(reconciled);
  assert.equal(cells.length, 1, '对账不得复制出第二个单元');
  assert.equal(cells[0].status, 'settled', '单元状态必须随权威记录收敛');
  assert.deepEqual(cells[0].answers, [{ id: 'q1', selected: ['release'] }]);
  assert.equal(
    reconciled.pendingQuestions['call-909'],
    undefined,
    '已结算的提问不再占据待答位',
  );

  // 状态未变时保持原对象：对账每次进入车道都会跑，无谓的新对象会引起整段重渲染。
  const again = conversationReducer(reconciled, {
    type: 'reconcile-questions',
    authoritative: true,
    questions: [{ ...authoritativeRecord }],
  });
  assert.equal(questionCells(again)[0], cells[0], '状态未变时单元对象应保持同一份');
});
