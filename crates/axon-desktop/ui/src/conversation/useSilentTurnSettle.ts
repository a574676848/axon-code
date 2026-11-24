/*
 * 运行证据判过期后的自动收口。
 *
 * 界面认为这一轮还在跑、服务端却说没有带租约的执行者时，过去只有一条出路：给一张
 * 「运行证据已过期」的卡，让用户自己去重启后台服务。真实事故里后台是好的、这一轮
 * 也早就收口了，缺的只是那条收口帧；把这件事交给用户动手，等于让他去修一个没坏的东西。
 *
 * 服务端已经有幂等的显式收口接口（`settle-silent-turn`），并且回执区分四种情形：
 * 真的收口了、本来就不在途、此前已经收口、还有执行者。这里因此把顺序倒过来：
 *
 * 1. 证据判过期时不直接出卡，先按这一轮的 Interaction 标识问一次服务端；
 * 2. 服务端说已收口或本来就不在途：按它的判据在本地收口（计时停下、给一条结论行）；
 * 3. 服务端说还有执行者：上一份观测是旧读数，撤回结论，继续显示在跑；
 * 4. 只有这一次请求本身失败（网关读不到、路由不存在）才把卡交出去。
 *
 * 每一轮只问一次：判据落在服务端，重复问不会得到新结论，只会把网关压满。证据重新
 * 成立（服务端再次报在跑）时把这一轮的预算还回来，下一次过期仍然会核对。
 */

import { useEffect, useRef, useState } from 'react';

import { conversationSettleSilentTurn } from './sessionEndpoints';
import {
  liveEvidenceChecking,
  liveEvidenceRunning,
  type LiveEvidence,
} from './liveEvidence';

/** 一次核对的结论。 */
type SettleOutcome = 'idle' | 'checking' | 'running' | 'failed';

export interface SilentTurnSettleOptions {
  /** 当前车道标识；会话目标下为空。 */
  sessionId: string | null;
  /** 证据结论；只有 `expired` 触发核对。 */
  evidence: LiveEvidence;
  /** 还在跑的轮次的 Interaction 标识；乐观轮（回执未到）时为空，此时不核对。 */
  interactionId: string | null;
  /** 服务端已给出收口判据：交给控制器落终态。 */
  onSettled: (interactionId: string, verdict: string) => void;
}

/** 组装自动收口；返回视图真正该展示的证据。 */
export function useSilentTurnSettle(options: SilentTurnSettleOptions): LiveEvidence {
  const { sessionId, evidence, interactionId, onSettled } = options;
  /** 已经问过的那一轮：同一个 Interaction 只问一次。 */
  const askedRef = useRef<string | null>(null);
  const [outcome, setOutcome] = useState<SettleOutcome>('idle');
  /**
   * 回调走 ref。
   *
   * 它每次渲染都会换身份，进依赖表会让核对的 effect 每帧重跑；而重跑时预算已经用掉，
   * 那次核对的结论就被丢掉了。
   */
  const settledRef = useRef(onSettled);
  settledRef.current = onSettled;

  useEffect(() => {
    if (sessionId === null || interactionId === null) {
      askedRef.current = null;
      setOutcome('idle');
      return;
    }
    if (evidence.state !== 'expired') {
      // 证据重新成立：这一轮的核对预算还回来，下一次过期仍要核对一次。
      if (evidence.state === 'running') askedRef.current = null;
      setOutcome((previous) => (previous === 'idle' ? previous : 'idle'));
      return;
    }
    if (askedRef.current === interactionId) return;
    askedRef.current = interactionId;
    setOutcome('checking');
    let cancelled = false;
    conversationSettleSilentTurn(sessionId, interactionId)
      .then((receipt) => {
        if (cancelled) return;
        if (receipt.verdict === 'still_running') {
          // 服务端还在跑：上一份读数过期了，不是这一轮没了。
          setOutcome('running');
          return;
        }
        setOutcome('idle');
        settledRef.current(interactionId, receipt.verdict);
      })
      .catch(() => {
        // 只有这一次请求本身失败才把卡交出去：这是唯一需要人来处理的那一档。
        if (!cancelled) setOutcome('failed');
      });
    return () => {
      cancelled = true;
    };
  }, [evidence.state, interactionId, sessionId]);

  if (sessionId === null) return evidence;
  if (evidence.state !== 'expired') return evidence;
  /*
   * 首帧就压住卡片：核对在 effect 里发起，等到结论回来再渲染就会先闪一张卡。
   * 这一帧还不是「核对中」这个状态落定，只是「马上要核对」，因此不依赖 outcome。
   */
  if (outcome === 'failed') return evidence;
  if (interactionId === null) return evidence;
  if (outcome === 'running') return liveEvidenceRunning();
  return liveEvidenceChecking();
}
