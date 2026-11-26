/*
 * 投影入口。
 *
 * 会话外壳只与这一层打交道：一个纯函数吃旧状态与新动作，吐出状态。动作分三类，
 * 本地的显式操作（提交、引导、重置）、服务端身份绑定（交互标识、历史索引）与
 * 订阅流的帧批量。三者都不维护第二份可写副本，界面看到的永远是这份状态。
 */

import { applyFrames } from './frames';
import { collapseTurn, foldTurn, replaceTurnCells } from './fold';
import { applySettledSilentTurn } from './terminal';
import { INITIAL_CONVERSATION, LOCAL_STEER_PREFIX, type AttachmentRef, type ConversationAction, type ConversationState, type JobRow, type TurnState } from './state';
import { pruneSettledQuestions, type PendingQuestion } from '../questions';
import { emptyTurn, absorbClaimedUserCells, detachCells, dropStaleUndelivered, localUserId, openTurn, questionSettled, refreshLiveElapsed, refreshLiveSummary, upsertCell, withTurn } from './turns';

/** 归约入口。 */
export function conversationReducer(
  state: ConversationState,
  action: ConversationAction,
): ConversationState {
  switch (action.type) {
    case 'reset':
      /*
       * 清空投影，但推进投影代次。
       *
       * 会话级事实（问答对账）挂在代次上重取一次：清空之前取回的结论会被这一下原样
       * 抹掉，而代次不变就没有任何一处会再触发一次，待答位因此永远空着。
       */
      return { ...INITIAL_CONVERSATION, projectionGeneration: state.projectionGeneration + 1 };
    case 'plan-seed':
      // 快照只是「打开那一刻的当前计划」：已有计划（来自回放帧或实时帧）时一律不动。
      return state.plan === null ? { ...state, plan: action.plan } : state;
    case 'connecting':
      return { ...state, connection: 'connecting', failure: null };
    case 'open-turn':
      return dropStaleUndelivered(openTurn(state, action.turnId, action.nowMs), action.turnId);
    case 'submit':
      return submitTurn(state, action.turnId, action.text, action.nowMs, action.attachments);
    case 'steer':
      return steerTurn(state, action.text, action.nowMs, action.attachments, action.structuredInput);
    case 'detach-steers': {
      /*
       * 按标识从**所有轮次**里摘掉：转投时这些格子可能还在上一轮（被中断的那一轮），
       * 而此刻最新那一轮已经开跑。只摘「当前轮」的话，旧轮次里的副本会一直留着，
       * 新轮次再收一份，正文里就出现两条一模一样的消息。
       */
      const dropping = new Set(action.cellIds);
      let changed = false;
      const turns = state.turns.map((turn) => {
        if (!turn.cells.some((cell) => dropping.has(cell.id))) return turn;
        changed = true;
        return detachCells(turn, action.cellIds);
      });
      return changed ? { ...state, turns } : state;
    }
    case 'mark-steers-requeued': {
      const position = activePosition(state);
      if (position < 0) return state;
      const handingOver = new Set(action.cellIds);
      return withTurn(state, position, (turn) => {
        let changed = false;
        const cells = turn.cells.map((cell) => {
          if (cell.role !== 'user' || !handingOver.has(cell.id) || cell.requeued === true) return cell;
          changed = true;
          return { ...cell, requeued: true };
        });
        return changed ? { ...turn, cells } : turn;
      });
    }
    case 'submit-failed':
      return failSubmit(state, action.turnId, action.text, action.message);
    case 'bind':
      return bindTurn(state, action.turnId, action.interactionId);
    case 'attach-history':
      return attachHistory(
        state,
        action.interactionId,
        action.attachments,
        action.channel,
      );
    case 'seed-jobs': {
      // 会话级清单只补空位：帧里的行带着命令与输出，比索引更完整，不能被覆盖。
      const jobs = mergeJobs(state.jobs, action.jobs);
      return jobs === state.jobs ? state : { ...state, jobs };
    }
    case 'prepend-turns': {
      if (action.turns.length === 0) return state;
      /*
       * 去重后再接。
       *
       * 前置进来的既可能是回放算好的一页，也可能是重连后再取的一次首屏页：
       * 同一条 Interaction 只能有一个轮次，重复接进头部就是两份同样的历史。
       * 作业台账随页一起带过来：它不随单元释放，因此必须在这里合并。
       */
      const fresh = unseenTurns(state, action.turns);
      const jobs = mergeJobs(state.jobs, action.jobs);
      if (fresh.length === 0 && jobs === state.jobs) return state;
      return absorbClaimedUserCells(
        {
          ...state,
          turns: fresh.length === 0 ? state.turns : [...fresh, ...state.turns],
          jobs,
        },
        fresh,
      );
    }
    case 'append-turns': {
      const fresh = unseenTurns(state, action.turns);
      // 增量接管只接还不认识的轮次：外部事件到达时整屏正文原地延长，不重排。
      const jobs = mergeJobs(state.jobs, action.jobs);
      if (fresh.length === 0 && jobs === state.jobs) return state;
      return absorbClaimedUserCells(
        {
          ...state,
          turns: fresh.length === 0 ? state.turns : [...state.turns, ...fresh],
          jobs,
        },
        fresh,
      );
    }
    case 'fold-turn': {
      // 折叠即释放：过程单元在这一步被撤下，控件与内容由 foldTurn 自己算出来。
      const position = state.turns.findIndex((turn) => turn.id === action.turnId);
      return position < 0 ? state : withTurn(state, position, (turn) => foldTurn(turn));
    }
    case 'collapse-turn': {
      const position = state.turns.findIndex((turn) => turn.id === action.turnId);
      if (position < 0) return state;
      return withTurn(state, position, (turn) => collapseTurn(turn));
    }
    case 'replace-turn': {
      // 回放算出来的待答提问一并落回待答位：单元与作答入口是同一件事实的两处投影。
      const base = adoptReplayedQuestions(state, action.questions);
      const position = base.turns.findIndex((turn) => turn.id === action.turn.id);
      if (position < 0) {
        /*
         * 这一轮不在时间线上，但帧已经证明它存在：补进来，不能静默丢弃。
         *
         * 索引骨架会过滤「审计轮」（agent 通道、已收口、没有正文），免得把派发记录当成
         * 一轮对话摆进正文。可子代理会话唯一的一轮就是这种形状：骨架里没有它，随后按帧
         * 投影出来的整轮又因为找不到位置被丢掉，于是正文永远空白。判据放在这里最稳：
         * 有帧就是事实，按起始时刻插到正确位置；没有任何帧时仍然不会凭空长出一轮。
         */
        const inserted = [...base.turns, action.turn].sort(
          (left, right) => (left.startedAtMs ?? 0) - (right.startedAtMs ?? 0),
        );
        const jobs = mergeJobs(base.jobs, action.jobs);
        const withJobs = jobs === base.jobs ? base : { ...base, jobs };
        return absorbClaimedUserCells({ ...withJobs, turns: inserted }, [action.turn]);
      }
      const replaced = withTurn(base, position, () => {
        const incoming = { ...action.turn, folded: false, summary: action.turn.summary };
        if (action.mode === 'folded') {
          // 折叠位置直接用原始单元折：先插控件再折会给「没有过程」的轮次留下一根空抬头。
          return foldTurn({ ...incoming, keepExpanded: false, expanded: false, summary: '' });
        }
        if (action.mode === 'expanded') {
          return replaceTurnCells(incoming, incoming.cells);
        }
        return incoming;
      });
      const jobs = mergeJobs(replaced.jobs, action.jobs);
      const merged = jobs === replaced.jobs ? replaced : { ...replaced, jobs };
      /*
       * 整轮替换是按索引与帧投影出来的，途中没有机会走 `applyUserMessage` 的按正文认领：
       * 这条消息若原本在别的轮次里有一格乐观占位（服务端没注入当时那一轮，而是另开了一
       * 轮），那一格会原地留下并在收口时标成未送达，正文里同一条消息就成了两格气泡。
       */
      return absorbClaimedUserCells(merged, [action.turn]);
    }
    case 'tick':
      // 耗时走字的同时刷新分割线：生成中的读数一直在，不必等收口。
      return {
        ...state,
        turns: state.turns.map((turn) =>
          refreshLiveSummary(refreshLiveElapsed(turn, action.nowMs), action.nowMs),
        ),
      };
    case 'frames':
      return applyFrames(state, action.frames, action.nowMs);
    case 'settle-silent-turn':
      // 收口结论来自服务端判据：状态、计时与结论行都在这一处落定，界面不自造终态。
      return applySettledSilentTurn(state, action.interactionId, action.verdict, action.nowMs);
    case 'reconcile-questions':
      return reconcileQuestions(state, action.questions, action.authoritative === true);
    default:
      return state;
  }
}

/**
 * 把一次回放投影出来的待答提问落进会话级待答位。
 *
 * 只补不覆盖：实时帧与对账都比一次历史回放新，已有条目的状态不得被更旧的帧改回去。
 * 已结算的条目不入位（待答位只关心还需要回答的那几条），台账里已有权威结论的同样
 * 不入位。补进去之后，重进会话时的占座卡与时间线那条问答单元由同一份回放事实成立，
 * 不必再靠一次额外往返把作答入口找回来。
 */
function adoptReplayedQuestions(
  state: ConversationState,
  questions: readonly PendingQuestion[] | undefined,
): ConversationState {
  if (questions === undefined || questions.length === 0) return state;
  let pending = state.pendingQuestions;
  let changed = false;
  for (const question of questions) {
    if (question.status === 'settled') continue;
    if (pending[question.callId] !== undefined) continue;
    if (questionSettled(state, question.callId)) continue;
    if (!changed) {
      pending = { ...pending };
      changed = true;
    }
    pending[question.callId] = question;
  }
  return changed ? { ...state, pendingQuestions: pending } : state;
}

/**
 * 对账：用服务端权威记录校正本地待答状态。
 *
 * 只覆盖对账真正携带的字段。网关不提供 partId，本地从事件流得知的标识必须留住；
 * 问题列表与截止时间缺失时同理，否则一次对账会把用户正在看的提问抹成空白。
 * 结算过的条目在这一步就离场：对账是「现在还在等谁回答」的答案，答完的留在
 * 待答位里只会让标识永远挂在那里。
 */
function reconcileQuestions(
  state: ConversationState,
  questions: readonly PendingQuestion[],
  authoritative: boolean,
): ConversationState {
  const updated = { ...state.pendingQuestions };
  const observed = new Set<string>();
  for (const incoming of questions) {
    observed.add(incoming.callId);
    const previous = updated[incoming.callId];
    updated[incoming.callId] = previous === undefined ? incoming : mergeQuestion(previous, incoming);
  }
  if (authoritative) {
    for (const [callId, question] of Object.entries(updated)) {
      // 服务端已经不再列出的待决项：它被别的渠道答掉、取消或过期了。
      if (observed.has(callId) || question.status === 'settled') continue;
      updated[callId] = { ...question, status: 'settled' };
    }
  }
  /*
   * 时间线单元与待答位是同一事实的两处投影：单元在提问那一刻被物化进轮次，
   * 之后只靠 wire 帧刷新。迟到作答发生在回合之外，没有帧可依附，因此不在这里
   * 一并改写单元，界面就会一直显示作答前的旧状态，并给出注定被服务端拒绝的
   * 「重新打开」。单元写入用对账后的值，因此在清理已结算项之前先取。
   */
  const turns = state.turns.map((turn) => {
    let changed = false;
    const cells = turn.cells.map((cell) => {
      if (cell.role !== 'question') return cell;
      const known = updated[cell.callId];
      if (known === undefined) return cell;
      const answers = known.answers ?? cell.answers;
      if (known.status === cell.status && answers === cell.answers) return cell;
      changed = true;
      return { ...cell, status: known.status, answers };
    });
    return changed ? { ...turn, cells } : turn;
  });
  return {
    ...state,
    turns,
    resolvedQuestions: withResolvedQuestions(state.resolvedQuestions, updated),
    pendingQuestions: pruneSettledQuestions(updated),
  };
}

/**
 * 把已结算的结论记进台账。
 *
 * 对账是权威结论，但它的到达时刻不由界面决定：进入车道时它常常早于历史回放，
 * 那时时间线上还没有单元可改写。台账因此承担「结论先到、单元后到」的那一段：
 * 回放出来的挂起帧在这里被夹住，不再把答过的题摆回待答态。已经记过的条目保持
 * 原对象，避免每次对账都换一份状态引起整段重渲染。
 */
function withResolvedQuestions(
  ledger: ConversationState['resolvedQuestions'],
  questions: Readonly<Record<string, PendingQuestion>>,
): ConversationState['resolvedQuestions'] {
  let next = ledger;
  for (const [callId, question] of Object.entries(questions)) {
    if (question.status !== 'settled') continue;
    const known = next[callId];
    const answers = question.answers ?? known?.answers;
    if (known !== undefined && known.answers === answers) continue;
    if (next === ledger) next = { ...ledger };
    next[callId] = { answers };
  }
  return next;
}

/** 合并一条对账结果，保留本地已知而服务端未下发的字段。 */
function mergeQuestion(previous: PendingQuestion, incoming: PendingQuestion): PendingQuestion {
  return {
    ...previous,
    partId: previous.partId.length > 0 ? previous.partId : incoming.partId,
    status: incoming.status,
    questions: incoming.questions.length > 0 ? incoming.questions : previous.questions,
    answers:
      incoming.answers !== undefined && incoming.answers.length > 0
        ? incoming.answers
        : previous.answers,
    handoff: incoming.handoff ?? previous.handoff,
    deadlineAtMs: incoming.deadlineAtMs > 0 ? incoming.deadlineAtMs : previous.deadlineAtMs,
  };
}

/** 提交一条用户消息并立刻开一轮。 */
function submitTurn(
  state: ConversationState,
  turnId: string,
  text: string,
  nowMs: number,
  attachments: AttachmentRef[],
): ConversationState {
  const turn = upsertCell({ ...emptyTurn(turnId, nowMs), status: 'pending' },
    { id: localUserId(turnId), role: 'user', text, atMs: nowMs, attachments });
  return { ...state, turns: [...state.turns, turn], failure: null, connection: 'connecting' };
}

/**
 * 绑定服务端派生出来的交互标识。
 *
 * 服务端把输入注入到同车道正在运行的回合时回执里的标识与本地那一轮不同：
 * 此时并入已有轮次，不另起一轮，否则同一段对话会被拆成两条。
 */
function bindTurn(state: ConversationState, turnId: string, interactionId: string): ConversationState {
  const existing = state.turns.findIndex((turn) => turn.interactionId === interactionId);
  const local = state.turns.findIndex((turn) => turn.id === turnId);
  if (local < 0) return state;

  if (existing >= 0 && existing !== local) {
    const target = state.turns[existing];
    const merged = mergeUserCells(target, state.turns[local]);
    const turns = state.turns
      .map((turn, position) => (position === existing ? merged : turn))
      .filter((_, position) => position !== local);
    return { ...state, turns };
  }

  const turns = state.turns.map((turn, position) =>
    position === local ? { ...turn, id: interactionId, interactionId } : turn,
  );
  return { ...state, turns };
}

/** 并入另一轮里尚未出现的用户消息。 */
function mergeUserCells(target: TurnState, incoming: TurnState): TurnState {
  const known = new Set(target.cells.filter((cell) => cell.role === 'user').map((cell) => cell.text));
  const additions = incoming.cells.filter(
    (cell) => cell.role === 'user' && !known.has(cell.text),
  );
  if (additions.length === 0) return target;
  return { ...target, cells: [...target.cells, ...additions] };
}

/**
 * 运行中继续发送。
 *
 * 后端会把这条输入注入同车道正在运行的那一轮，并在安全边界（下一次 provider
 * 请求之前或工具批次收口处）插入对话，因此这里**不新开轮次**，只把用户消息
 * 追加到当前这一轮上；开新轮会让随后到达的流式帧被记到错误的轮次里。
 *
 * 结构化输入块跟着这一格走：中断时它要被原样转投到下一条消息上，而正文被裁成
 * 可见文本之后，引用行与附件行再也算不回来。
 */
function steerTurn(
  state: ConversationState,
  text: string,
  nowMs: number,
  attachments: AttachmentRef[],
  structuredInput: string,
): ConversationState {
  const position = activePosition(state);
  if (position < 0) return state;
  return withTurn(state, position, (turn) =>
    upsertCell(turn, {
      id: `${LOCAL_STEER_PREFIX}${turn.cells.length}-${nowMs}`,
      role: 'user',
      text,
      atMs: nowMs,
      attachments,
      structuredInput,
      steer: true,
      steerSeq: nextSteerSequence(),
    }),
  );
}

/**
 * 追加消息的先后序号。
 *
 * 只需要在同一个客户端里单调递增：它唯一的用途是把「用户敲的顺序」固定下来，
 * 因为认领会改写标识与时间，注入顺序也不保证与提交顺序一致。
 */
let steerSequence = 0;

function nextSteerSequence(): number {
  steerSequence += 1;
  return steerSequence;
}

/** 最后一条未收口轮次的位置；没有则取最后一条。 */
function activePosition(state: ConversationState): number {
  for (let position = state.turns.length - 1; position >= 0; position -= 1) {
    const status = state.turns[position].status;
    if (status === 'pending' || status === 'streaming') return position;
  }
  return state.turns.length - 1;
}

/** 同一标识的轮次只能有一份：接进时间线前先摘掉投影里已有的那些。 */
function unseenTurns(state: ConversationState, incoming: readonly TurnState[]): TurnState[] {
  const known = new Set(state.turns.map((turn) => turn.id));
  return incoming.filter((turn) => {
    if (known.has(turn.id)) return false;
    known.add(turn.id);
    return true;
  });
}

/** 合并一页带来的作业台账：同一条作业只保留一行，后到的覆盖先到的。 */
function mergeJobs(current: JobRow[], incoming: JobRow[] | undefined): JobRow[] {
  if (incoming === undefined || incoming.length === 0) return current;
  const index = new Map(current.map((row, position) => [row.id, position]));
  const next = [...current];
  for (const row of incoming) {
    const known = index.get(row.id);
    if (known === undefined) {
      index.set(row.id, next.length);
      next.push(row);
    } else {
      next[known] = row;
    }
  }
  return next;
}

/**
 * 用历史索引补齐回放轮次的附件与来源。
 *
 * 会话事件流只承载正文，附件与逐轮来源不在 wire 事件里；历史索引是两者唯一的事实源，
 * 因此每一次回放都要把索引里的附件路径与来源贴回对应的用户消息上。用户消息还没重放
 * 出来时先记在轮次上，等它落地再贴（见 `applyUserMessage`）。
 */
function attachHistory(
  state: ConversationState,
  interactionId: string,
  attachments: AttachmentRef[],
  channel: string | undefined,
): ConversationState {
  const position = state.turns.findIndex((turn) => turn.id === interactionId);
  if (position < 0) return state;
  return withTurn(state, position, (turn) => {
    let patched = false;
    const cells = turn.cells.map((cell) => {
      if (patched || cell.role !== 'user') return cell;
      patched = true;
      return {
        ...cell,
        attachments: attachments.length > 0 ? attachments : cell.attachments,
        channel: channel ?? cell.channel,
      };
    });
    // 附件还没落地时先记在轮次上；来源同理，等用户消息到达时一并贴上。
    return {
      ...turn,
      cells,
      pendingAttachments: patched ? [] : attachments,
      pendingChannel: channel ?? turn.pendingChannel,
    };
  });
}

/**
 * 提交失败：把乐观状态收回成「未送达」。
 *
 * 提交失败时这一轮永远不会开起来，若原样留着，界面会一直停在「生成回复中」，
 * 用户只能看到一个永远不动的耗时。因此：
 * - 运行中排队失败：把刚追加的那一格标成未送达，不新开轮次；
 * - 新开一轮失败：整轮收口为失败并补一条可读提示，消息本体标成未送达。
 */
function failSubmit(
  state: ConversationState,
  turnId: string | null,
  text: string,
  message: string,
): ConversationState {
  if (turnId !== null) {
    const position = state.turns.findIndex((turn) => turn.id === turnId);
    if (position >= 0) {
      return withTurn(state, position, (turn) => {
        const warned = upsertCell(markUndelivered(turn, text), {
          id: `submit-failed:${turnId}`,
          role: 'notice',
          title: '发送失败',
          summary: message,
          tone: 'danger',
        });
        // 这一轮从未到达后端，用独立的未送达终态收口：既不再显示「生成回复中」，
        // 也不会被当成一次真正的执行失败。
        return { ...warned, status: 'undelivered', activity: null, elapsedMs: null };
      });
    }
    return state;
  }
  const position = activePosition(state);
  return position < 0 ? state : withTurn(state, position, (turn) => markUndelivered(turn, text));
}

/** 把指定正文的乐观用户消息标成未送达；只动最后一格，避免误伤同文的历史消息。 */
function markUndelivered(turn: TurnState, text: string): TurnState {
  for (let index = turn.cells.length - 1; index >= 0; index -= 1) {
    const cell = turn.cells[index];
    if (cell.role !== 'user' || cell.text !== text || cell.undelivered) continue;
    const cells = [...turn.cells];
    cells[index] = { ...cell, undelivered: true };
    return { ...turn, cells };
  }
  return turn;
}
