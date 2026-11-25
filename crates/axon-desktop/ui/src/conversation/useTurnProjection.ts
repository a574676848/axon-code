/*
 * 一轮的投影：有帧就纯按帧投影，没有帧才用索引骨架。
 *
 * 从 `useSessionPaging` 里单独拿出来：打开会话、展开一轮与增量接管共用同一套口径，
 * 任何一条路径改了投影规则，另外两条自动跟上。取数（索引、帧、订阅）留在分页那一层，
 * 投影（这一轮长什么样）收在这里，两处各自只读自己那一半。
 *
 * 两条硬规则与 `historyProjection` 同源，这里只负责把它们装成回调：
 * - **两条路径不能叠**：索引骨架里那条合成的用户消息会与帧里的真实用户消息各成一格，
 *   而骨架自带的收口帧又会先把这一轮判成结束，让随后的回放帧另起一轮（实测表现为
 *   「打开会话后最新一轮的过程行整段不见、上下文读数也空着」）。因此这里二选一。
 * - **不替服务端收口**：还在跑的一轮必须留在 `streaming`，否则 `working` 当场变假，
 *   工作状态行与侧栏转圈一起消失。
 */

import { useCallback } from 'react';

import type { TurnMetricsReading } from '../types';
import {
  insertControlAfterInput,
  interactionStillRunning,
  projectFrames,
  readingFactsOf,
  turnSummaryFacts,
  type HistoryFacts,
} from './historyProjection';
import { isAuditTurn } from './pagingBounds';
import { provisionalFrames, type HistoryEntry } from './pagingSkeleton';
import type { PendingQuestion } from './questions';
import {
  INITIAL_CONVERSATION,
  applyFrames,
  conversationReducer,
  outlineControl,
  summaryFromIndexFacts,
  type ConversationAction,
  type JobRow,
  type TurnState,
} from './transcript';
import { applyTurnSummary, seedTurnReadings, settleTurn, type TurnSummaryFacts } from './transcript/turns';
import type { ConversationStreamEvent } from './wire';

/** 一轮投影的结果：轮次、作业台账，以及回放里见到的那几条待答提问。 */
export interface ProjectedTurn {
  turn: TurnState;
  jobs: JobRow[];
  /**
   * 回放见到的待答提问。
   *
   * 与时间线上那张问答单元是同一件事实的两处投影：只交轮次会让作答入口随一次性
   * 状态一起丢掉，重进会话时占座卡因此不出现。归约器按 `replace-turn` 的载荷收下它。
   */
  questions: PendingQuestion[];
}

/** 投影所需的取数事实：索引补充事实与逐轮读数都在分页那一层维护。 */
export interface TurnProjectionDeps {
  /** 投影入口：收口兜底要自己派发一次整轮替换。 */
  dispatch: React.Dispatch<ConversationAction>;
  /** 历史索引的补充事实表（附件与来源）。 */
  facts: React.RefObject<Map<string, HistoryFacts>>;
  /** 索引给出的逐轮读数；`turn_metrics` 是这一轮读数的权威事实。 */
  readings: React.RefObject<Map<string, TurnMetricsReading>>;
}

/** 对外交出的三个投影入口。 */
export interface TurnProjection {
  /** 把一轮投影出来（有帧按帧，没有帧按索引骨架）。 */
  projectTurn: (
    entry: HistoryEntry,
    frames: readonly ConversationStreamEvent[] | null,
    truncated?: boolean,
    settled?: boolean,
  ) => ProjectedTurn;
  /** 一条轮次的骨架形态（打开时光标还没落到任何历史帧上）。 */
  withProvisional: (entries: readonly HistoryEntry[]) => TurnState[];
  /** 帧读不到时的收口兜底。 */
  settleUnread: (entry: HistoryEntry) => void;
}

/**
 * 装配投影入口。
 *
 * `facts` 与 `readings` 用 ref 传进来而不是当依赖：索引每次翻页都会重写这两张表，
 * 若把它们摊成依赖，每个用到投影的回调都会随翻页重建一次。
 */
export function useTurnProjection({ dispatch, facts, readings }: TurnProjectionDeps): TurnProjection {
  /**
   * 一轮的分割线读数：索引给出的 `turn_metrics` 优先，缺失时退回帧算出来的那份。
   */
  const readingFactsFor = useCallback(
    (interactionId: string, fallbackElapsedMs: number | null, turn: TurnState): TurnSummaryFacts =>
      turnSummaryFacts(readings.current.get(interactionId), fallbackElapsedMs, turn),
    [readings],
  );

  /** 索引读数：把 `turn_metrics` 换成归约器认得的形状，供指标条在帧回来之前成立。 */
  const indexReadings = useCallback(
    (interactionId: string): ReturnType<typeof readingFactsOf> =>
      readingFactsOf(readings.current.get(interactionId)),
    [readings],
  );

  /**
   * 把一轮投影出来：有帧就纯按帧投影，没有帧才用索引骨架。
   *
   * `truncated` 是回放自己交回来的事实：这一轮中间一段没读到，正文要如实标一句。
   * `settled` 是这一轮在服务端是否已经收口：有帧时由 `InteractionFrames.settled` 给
   * （帧里有没有终态帧），没有帧的骨架路径按索引里的 lifecycle 判。
   */
  const projectTurn = useCallback(
    (
      entry: HistoryEntry,
      frames: readonly ConversationStreamEvent[] | null,
      truncated = false,
      settled = !interactionStillRunning(entry.status),
    ): ProjectedTurn => {
      if (frames !== null && frames.length > 0) {
        return projectFrames(
          entry.interactionId,
          frames,
          facts.current.get(entry.interactionId),
          readings.current.get(entry.interactionId),
          entry.elapsedMs ?? null,
          { truncated, settled },
        );
      }
      let state = conversationReducer(INITIAL_CONVERSATION, {
        type: 'open-turn',
        turnId: entry.interactionId,
        nowMs: entry.receivedAtMs,
      });
      const history = facts.current.get(entry.interactionId);
      if (history !== undefined) {
        state = conversationReducer(state, {
          type: 'attach-history',
          interactionId: entry.interactionId,
          attachments: history.attachments,
          channel: history.channel,
        });
      }
      state = applyFrames(state, provisionalFrames(entry), entry.receivedAtMs);
      const turn = state.turns[0];
      if (turn === undefined) {
        return { turn: state.turns[0], jobs: state.jobs, questions: [] };
      }
      const closed =
        settled && (turn.status === 'streaming' || turn.status === 'pending')
          ? settleTurn(turn, 'completed', entry.elapsedMs ?? null)
          : turn;
      /*
       * 索引读数先落到这一轮上：首字、速率与用量在帧回来之前就要在指标条上成立。
       * 窗口占用不在索引里，因此这一轮只有 token 分桶，上下文段仍等收尾负载。
       */
      const seeded = seedTurnReadings(closed, indexReadings(entry.interactionId));
      /*
       * 只读索引的那一轮：给一根折叠行当入口。
       *
       * 过程要等点开才读整轮，但读者需要一个「这一轮可以展开」的落点；标题只写中性的
       * 「过程」，点开之后换成帧算出来的真实动作摘要（没有过程时这一行会消失）。
       */
      const elapsed = readingFactsFor(entry.interactionId, entry.elapsedMs ?? null, seeded).durationMs;
      /*
       * 没有过程证据就不立折叠行。
       *
       * 纯问答轮点开也没有过程可看，给它立一根「过程」抬头只是把一次问答伪装成跑过
       * 东西的回合。判据取索引里的事实：这一轮有工具名、有工具调用次数或有推理输出，
       * 三者都没有才认定它没有过程。
       */
      const reading = readings.current.get(entry.interactionId);
      const hasProcess =
        (entry.toolNames?.length ?? 0) > 0 ||
        (reading?.toolCalls ?? 0) > 0 ||
        (reading?.reasoningTokens ?? 0) > 0;
      const withControl = hasProcess
        ? insertControlAfterInput(
            seeded,
            outlineControl(
              entry.interactionId,
              entry.interactionId,
              entry.receivedAtMs,
              elapsed,
              summaryFromIndexFacts(entry.toolNames ?? [], reading?.reasoningTokens),
            ),
          )
        : seeded;
      return {
        turn: applyTurnSummary(
          withControl,
          readingFactsFor(entry.interactionId, entry.elapsedMs ?? null, withControl),
        ),
        jobs: state.jobs,
        // 只有索引的骨架里没有任何帧，因此也没有可交回的待答提问。
        questions: [],
      };
    },
    [facts, indexReadings, readingFactsFor, readings],
  );

  /** 一条轮次的骨架形态（打开时光标还没落到任何历史帧上）。 */
  const withProvisional = useCallback(
    (entries: readonly HistoryEntry[]) =>
      entries.filter((entry) => !isAuditTurn(entry)).map((entry) => projectTurn(entry, null).turn),
    [projectTurn],
  );

  /**
   * 帧读不到时的收口兜底。
   *
   * 骨架已经按「在跑」立起来了，但这一轮此刻没有任何证据（帧没读到，实时尾也没订上）：
   * 继续挂着 `streaming` 会让工作状态行与侧栏转圈永远转下去，比少一段过程更误导。
   * 退回按历史收口的骨架，失败本身仍由调用方如实上报。
   */
  const settleUnread = useCallback(
    (entry: HistoryEntry) => {
      const settled = projectTurn(entry, null, false, true);
      dispatch({
        type: 'replace-turn',
        turn: settled.turn,
        jobs: settled.jobs,
        questions: settled.questions,
      });
    },
    [dispatch, projectTurn],
  );

  return { projectTurn, withProvisional, settleUnread };
}
