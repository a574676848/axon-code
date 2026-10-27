import React from 'react';
import { isDesktopHost } from '../../backend';
import { createVoiceCapture, type VoiceCapture } from '../../speech/capture';
import {
  joinDraft,
  phaseFromServerState,
  type VoicePhase,
  type VoiceServerFrame,
} from '../../speech/voiceProtocol';
import { listenVoiceSession, startVoiceSession, stopVoiceSession } from '../../speech/voiceSession';
import { useExpiringNotice } from './composerNotice';

/*
 * 输入坞的语音控制器。
 *
 * 采集、协议与正文写入分属三层：采样与合帧在 capture，服务端帧到六态的映射在
 * voiceProtocol，本 hook 只编排「开始、结束、收帧」这三件事。定稿一律经既有
 * useComposerEditing.commit 追加，因此草稿与撤销历史都保留，语音不另开一份正文。
 *
 * 播放与打断属于后续增量，本切片不实现：speaking 与 interrupted 只在真的收到
 * 对应相位帧时出现，界面不自行造态。
 */

/** 结束采集后等待定稿的上限；超时把入口复位，不留在思考态。 */
const FINALIZE_TIMEOUT_MS = 8_000;

export interface ComposerVoiceOptions {
  /** 输入坞是否可提交；不可提交时语音也不可用。 */
  enabled: boolean;
  /** 当前工作区；缺失时不能建立语音会话。 */
  workspaceId: string | null;
  /** 当前会话的回复目的地；缺失时不能建立语音会话。 */
  replyDestination: string;
  /** 当前草稿；定稿追加在它之后。 */
  text: string;
  /** 既有编辑控制器写入入口。 */
  commit: (next: string) => void;
}

export interface ComposerVoice {
  /** 麦克风入口是否可用。 */
  available: boolean;
  /** 界面六态。 */
  phase: VoicePhase;
  /** 服务端中间识别结果；空串表示没有。 */
  partial: string;
  /** 一次性失败结论。 */
  error: string | null;
  /** 本次采集累计丢弃的帧数。 */
  droppedFrames: number;
  /** 采集是否处于打开状态。 */
  active: boolean;
  /** 点击麦克风。 */
  toggle: () => void;
}

/** 把未知异常翻成中文结论。 */
function describeFailure(failure: unknown): string {
  if (failure instanceof Error && failure.message.length > 0) return failure.message;
  if (typeof failure === 'string' && failure.length > 0) return failure;
  return '语音输入失败，请重试';
}

export function useComposerVoice({
  enabled,
  workspaceId,
  replyDestination,
  text,
  commit,
}: ComposerVoiceOptions): ComposerVoice {
  const [phase, setPhase] = React.useState<VoicePhase>('idle');
  const [partial, setPartial] = React.useState('');
  const [error, setError] = React.useState<string | null>(null);
  const [active, setActive] = React.useState(false);
  const [droppedFrames, setDroppedFrames] = React.useState(0);
  const captureRef = React.useRef<VoiceCapture | null>(null);
  /** 会话代次：连点或中途取消时，旧的异步启动不得覆盖新状态。 */
  const generationRef = React.useRef(0);
  const finalizeTimer = React.useRef<number | null>(null);
  const textRef = React.useRef(text);
  textRef.current = text;
  const commitRef = React.useRef(commit);
  commitRef.current = commit;

  /** 失败结论按一次性回执处理，到点自己退场。 */
  useExpiringNotice(error, () => setError(null));

  const clearFinalizeTimer = React.useCallback(() => {
    if (finalizeTimer.current === null) return;
    window.clearTimeout(finalizeTimer.current);
    finalizeTimer.current = null;
  }, []);

  const stopCapture = React.useCallback(() => {
    captureRef.current?.stop();
    captureRef.current = null;
    setActive(false);
  }, []);

  const fail = React.useCallback(
    (message: string) => {
      generationRef.current += 1;
      clearFinalizeTimer();
      stopCapture();
      setPhase('idle');
      setPartial('');
      setError(message);
    },
    [clearFinalizeTimer, stopCapture],
  );

  const handleFrame = React.useCallback(
    (frame: VoiceServerFrame) => {
      switch (frame.type) {
        case 'ready':
          setPhase((current) => (current === 'idle' ? 'arming' : current));
          break;
        case 'state': {
          const next = phaseFromServerState(frame.phase);
          setPhase(next);
          if (next === 'idle') setPartial('');
          break;
        }
        case 'partial':
          setPartial(frame.text ?? '');
          setPhase('listening');
          break;
        case 'final': {
          const finalText = (frame.text ?? '').trim();
          if (finalText.length > 0) {
            commitRef.current(joinDraft(textRef.current, finalText));
          }
          clearFinalizeTimer();
          stopCapture();
          setPartial('');
          setPhase('idle');
          break;
        }
        case 'error':
          fail(frame.message ?? '语音输入失败，请重试');
          break;
        case 'closed':
          clearFinalizeTimer();
          stopCapture();
          setPartial('');
          setPhase('idle');
          break;
        default:
          break;
      }
    },
    [clearFinalizeTimer, fail, stopCapture],
  );

  React.useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listenVoiceSession((frame) => {
      if (!disposed) handleFrame(frame);
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [handleFrame]);

  const stopListening = React.useCallback(() => {
    generationRef.current += 1;
    clearFinalizeTimer();
    stopCapture();
    setPhase('thinking');
    finalizeTimer.current = window.setTimeout(() => {
      finalizeTimer.current = null;
      setPhase('idle');
      setPartial('');
    }, FINALIZE_TIMEOUT_MS);
    void stopVoiceSession().catch((failure) => fail(describeFailure(failure)));
  }, [clearFinalizeTimer, fail, stopCapture]);

  const startListening = React.useCallback(async () => {
    if (!enabled || !isDesktopHost() || workspaceId === null || replyDestination.length === 0) {
      return;
    }
    const generation = generationRef.current + 1;
    generationRef.current = generation;
    setError(null);
    setPartial('');
    setDroppedFrames(0);
    setPhase('arming');
    setActive(true);
    const capture = createVoiceCapture({
      onError: (message) => {
        if (generationRef.current === generation) fail(message);
      },
      onDropped: (total) => {
        if (generationRef.current === generation) setDroppedFrames(total);
      },
    });
    captureRef.current = capture;
    try {
      await capture.start();
      if (generationRef.current !== generation) {
        capture.stop();
        void stopVoiceSession().catch(() => undefined);
        return;
      }
      await startVoiceSession(workspaceId, replyDestination);
      if (generationRef.current !== generation) {
        capture.stop();
        void stopVoiceSession().catch(() => undefined);
        return;
      }
      capture.setForwarding(true);
      setPhase('listening');
    } catch (failure) {
      if (generationRef.current !== generation) return;
      capture.stop();
      captureRef.current = null;
      setActive(false);
      setPhase('idle');
      setError(describeFailure(failure));
    }
  }, [enabled, fail, replyDestination, workspaceId]);

  const toggle = React.useCallback(() => {
    if (active) stopListening();
    else void startListening();
  }, [active, startListening, stopListening]);

  React.useEffect(() => {
    if (availableGuard(enabled, workspaceId, replyDestination) || !active) return;
    stopListening();
  }, [active, enabled, replyDestination, stopListening, workspaceId]);

  React.useEffect(
    () => () => {
      generationRef.current += 1;
      clearFinalizeTimer();
      captureRef.current?.stop();
      captureRef.current = null;
      void stopVoiceSession().catch(() => undefined);
    },
    [clearFinalizeTimer],
  );

  return {
    available: availableGuard(enabled, workspaceId, replyDestination),
    phase,
    partial,
    error,
    droppedFrames,
    active,
    toggle,
  };
}

/** 麦克风入口的可用条件：桌面宿主、有工作区与回复目的地。 */
function availableGuard(
  enabled: boolean,
  workspaceId: string | null,
  replyDestination: string,
): boolean {
  return enabled && isDesktopHost() && workspaceId !== null && replyDestination.length > 0;
}
