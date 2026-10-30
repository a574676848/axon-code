import { useCallback, useEffect, useRef, useState } from 'react';
import type { Dispatch, RefObject } from 'react';
import {
  mergeCarriedPrompts,
  requeueableSteers,
  type AttachmentRef,
  type CarriedPrompt,
  type ConversationAction,
  type TurnState,
} from './transcript';

/*
 * 中断转投：把还没被注入本轮的排队消息交给下一轮。
 *
 * 与 TUI 的 `requeue_pending_steers_for_next_turn` 同一套语义：后端只在安全边界
 * 把运行中追加的输入插进本轮，而中断恰好让那次边界不会再发生，输入就会随本轮
 * 一起消失。因此按下中断的那一刻先把它们从本轮接下（气泡原地留驻并标成转投中），
 * 等这一轮真的收口、车道也真正空闲之后，整队并成一次提交送出，用户不必再敲一次。
 *
 * 单独成 hook 的原因只有一个：这套时序（交接、等收口、等车道空闲、整队一次提交）
 * 有四个必须一起成立的约束，混在会话编排里看不清边界。
 */

/** 车道空闲探测的重试间隔与次数：取不到空闲结论时宁可晚一点发，也不早发。 */
const LANE_IDLE_PROBE_INTERVAL_MS = 150;
const LANE_IDLE_PROBE_ATTEMPTS = 20;

/** 转投所需的最小依赖面。 */
export interface InterruptedCarryWiring {
  /** 队列归属的目标；目标一换，整队作废（送去下一条车道就是发错人）。 */
  targetKey: string | null;
  /** 是否还有轮次在跑：被中断的那一轮收口之前不能开始转投。 */
  working: boolean;
  /** 归约器当前轮次快照：要摘下哪些消息以它为准。 */
  turnsRef: RefObject<TurnState[]>;
  dispatch: Dispatch<ConversationAction>;
  /** 与原样重发一致：正文、附件与结构化输入块三项都照旧。 */
  submit: (text: string, attachments: AttachmentRef[], structuredInput: string) => Promise<void>;
  /**
   * 这条车道上是否已经没有活跃轮次。
   *
   * 本地收到终态帧不等于后端已经摘掉活跃轮次登记项，两者之间有几十毫秒的窗口；
   * 那一刻提交的输入会被注入那条正在收尾的轮次，随它一起作废（实测：三条排队
   * 消息按下中断后原样贴在已中断的那一轮里，一条都没跑起来）。
   */
  laneIdle: () => Promise<boolean>;
}

/** 组装转投队列；队列本身只在内存里，不写进正文投影。 */
export function useInterruptedCarry({
  targetKey,
  working,
  turnsRef,
  dispatch,
  submit,
  laneIdle,
}: InterruptedCarryWiring): () => void {
  const [queued, setQueued] = useState<CarriedPrompt[]>([]);
  /** 转投的一次提交是否在途：在途期间不允许再取队列头，否则会重复发送。 */
  const inFlightRef = useRef(false);
  /** 这一批是否已经开始往外送：批次一开始，后面几条不再等各自那一轮收口。 */
  const drainingRef = useRef(false);
  /** 队列当前属于哪个目标；与本次渲染的目标不一致时，这一队消息已经没有意义。 */
  const ownerRef = useRef(targetKey);

  /** 中断之前调用：把还没被注入的排队消息交给下一轮。 */
  const handOver = useCallback(() => {
    const carried = requeueableSteers(turnsRef.current);
    if (carried.length === 0) return;
    // 新的一次中断：它那一轮同样要先收口，转投才能开始。批次标记因此归零，
    // 否则刚被中断的这一轮还在收尾时就会把消息发进去，又一次落进死轮。
    drainingRef.current = false;
    /*
     * 气泡原地留着标成「转投下一条」，不在这里摘掉：一摘就要等到车道空闲、新轮次
     * 真的开跑之后才回来，中间那一段正文里这几条消息是不存在的，读起来就是
     * 「先清空再重写」。真正换轮次发生在下一条消息提交的那一帧，两个动作在同一
     * 个同步批次里落地，用户看不到空档。
     */
    dispatch({ type: 'mark-steers-requeued', cellIds: carried.map((item) => item.cellId) });
    setQueued((queue) => [...queue, ...carried]);
  }, [dispatch, turnsRef]);

  /*
   * 一次中断把整队消息**并成一次提交**，绝不让用户为第二条再按一次中断。
   *
   * 后端一次提交只接受一条消息，而往运行中的轮次注入只在安全边界发生：逐条发送
   * 既保不住到达顺序，后面几条还会落在模型已经开始思考之后。并成一条提交则是
   * 一次原子的输入：顺序、位置与「回复在气泡下面」同时成立，也不会有第二条要等。
   */
  useEffect(() => {
    if (ownerRef.current !== targetKey) {
      // 换了目标：剩下的消息属于上一条车道，不能转投到这一条。
      ownerRef.current = targetKey;
      setQueued((queue) => (queue.length === 0 ? queue : []));
      return;
    }
    if (queued.length === 0) return;
    if (inFlightRef.current || working) return;
    const batch = queued;
    inFlightRef.current = true;
    void (async () => {
      try {
        // 车道真正空闲才发：本地终态帧比后端摘掉登记项早，早发的输入会被注入死轮。
        for (let attempt = 0; attempt < LANE_IDLE_PROBE_ATTEMPTS; attempt += 1) {
          if (await laneIdle()) break;
          await wait(LANE_IDLE_PROBE_INTERVAL_MS);
        }
        const merged = mergeCarriedPrompts(batch);
        if (merged === null) return;
        // 气泡换到新轮次与提交在同一个同步批次里完成，因此没有「先清空再重写」的空档。
        dispatch({ type: 'detach-steers', cellIds: batch.map((item) => item.cellId) });
        await submit(merged.text, merged.attachments, merged.structuredInput);
      } catch {
        // 失败原因已经由提交链路亮在正文与状态行上，这里只负责推进队列。
      } finally {
        inFlightRef.current = false;
        setQueued((queue) => (queue.length === 0 ? queue : []));
      }
    })();
  }, [laneIdle, queued, submit, targetKey, working]);

  return handOver;
}

/** 等一会儿；只用于车道空闲探测的退避。 */
function wait(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}