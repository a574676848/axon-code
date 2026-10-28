import React from 'react';
import { isDesktopHost } from '../../backend';
import * as ipc from '../../ipc';
import { createVoiceCapture, type VoiceCapture } from '../../speech/capture';
import { selectedDeviceId } from '../../speech/devices';
import { resetLevel } from '../../speech/levelBus';
import { createVoiceStreamPlayer, type VoiceStreamPlayer } from '../../speech/streamPlayer';
import { rememberStreamedTurn } from '../../speech/streamedTurns';
import {
  describeVoiceError,
  joinDraft,
  phaseFromServerState,
  type VoicePhase,
  type VoiceServerFrame,
} from '../../speech/voiceProtocol';
import { listenVoiceSession, startVoiceSession, stopVoiceSession } from '../../speech/voiceSession';
import { isBlockedBySystem, readMicrophoneAccess, readVoiceRuntime, VOICE_RUNTIME_MISSING_TEXT } from '../../speech/voicePermission';
import { VOICE_UI_ENABLED } from '../../speech/enabled';
import { useVoicePermission, type ComposerPermissionPrompt, type VoiceMode } from './useVoicePermission';
import { useExpiringNotice } from './composerNotice';

/*
 * 语音控制器：听写与语音对话两种模式共用一套采集、授权与状态机。
 *
 * 采集、协议与写入分属三层：采样与合帧在 capture，服务端帧到六态的映射在
 * voiceProtocol，本 hook 编排「授权、开始、结束、收帧、关闭」。
 *
 * 两种模式的差别只有一处，即定稿的落点：
 * - **听写**（`dictation`，输入坞默认）：定稿追加进草稿，由人决定是否发送；
 * - **语音对话**（`conversation`）：网关把定稿提交成一轮，界面接管该回合、
 *   订阅增量，并按需把下行 `0x02` 音频连续排程播放。
 *
 * 授权是原生的一步：WebView 层已由宿主对本地受信任源静默放行，因此这里只需要
 * 在首次使用时给出一次应用内说明，并在操作系统隐私层拒绝时把用户送到系统设置。
 * 不出现浏览器式权限弹窗，也不假装失败重试就能自己好起来。
 */

/** 关闭语音后等待定稿的上限；超时静默收口，不把入口挂在等待态。 */
const FINALIZE_TIMEOUT_MS = 8_000;

/** 状态行的计时刷新间隔。 */
const ELAPSED_TICK_MS = 200;

/**
 * 单次录音上限。
 *
 * 与网关的会话时长同一口径（120 秒）。到点自动停止并定稿，避免用户在不知情时
 * 一直录下去、最后拿到一条被服务端拒绝的长音频。
 */
const MAX_RECORDING_MS = 120_000;

/** 首屏起语音时临时建立的车道目标。 */
export interface VoiceEntry {
  workspaceId: string;
  replyDestination: string;
}

export type { ComposerPermissionPrompt, VoiceMode } from './useVoicePermission';

export interface ComposerVoiceOptions {
  /** 输入坞是否可提交；不可提交时语音也不可用。 */
  enabled: boolean;
  /** 当前工作区；缺失时不能建立语音会话。 */
  workspaceId: string | null;
  /** 当前会话的回复目的地；首屏为空时交给 `ensureSession`。 */
  replyDestination: string;
  /** 当前草稿；听写定稿与拒收回退都写到这里。 */
  text: string;
  /** 既有编辑控制器写入入口。 */
  commit: (next: string) => void;
  /** 定稿落点；缺省是听写。 */
  mode?: VoiceMode;
  /** 语音对话模式下是否让本轮回复由语音流直接朗读。 */
  readAloud?: boolean;
  /** 首屏没有车道时先建一条，返回可用的会话目标。 */
  ensureSession?: () => Promise<VoiceEntry | null>;
  /** 语音对话模式：网关为这一轮分配了 Interaction。 */
  onTurn?: (interactionId: string, utterance: string) => void;
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
  /** 采集是否处于打开状态。 */
  active: boolean;
  /** 当前相位已持续多久；待机为零。 */
  elapsedMs: number;
  /** 授权引导；为空时不显示。 */
  permission: ComposerPermissionPrompt | null;
  /** 点击麦克风：开始语音，或关闭当前语音（关闭即定稿）。 */
  toggle: () => void;
  /** 停止采集并让服务端定稿，定稿写进输入坞。 */
  finish: () => void;
  /** 取消本次语音并丢弃识别结果。 */
  cancel: () => void;
  /** 用户在首次说明里确认允许。 */
  confirmPermission: () => void;
  /** 关掉授权引导。 */
  dismissPermission: () => void;
  /** 打开系统麦克风隐私设置。 */
  openSettings: () => void;
  /** 重新检测系统权限。 */
  retryPermission: () => void;
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
  mode = 'dictation',
  readAloud = false,
  ensureSession,
  onTurn,
}: ComposerVoiceOptions): ComposerVoice {
  const [phase, setPhase] = React.useState<VoicePhase>('idle');
  const [partial, setPartial] = React.useState('');
  const [error, setError] = React.useState<string | null>(null);
  const [active, setActive] = React.useState(false);
  const [elapsedMs, setElapsedMs] = React.useState(0);
  const captureRef = React.useRef<VoiceCapture | null>(null);
  const playerRef = React.useRef<VoiceStreamPlayer | null>(null);
  /** 会话代次：连点或中途关闭时，旧的异步启动不得覆盖新状态。 */
  const generationRef = React.useRef(0);
  const finalizeTimer = React.useRef<number | null>(null);
  const elapsedTimer = React.useRef<number | null>(null);
  const startedAt = React.useRef(0);
  /** 停在等待定稿的文本；听写模式收口时写进草稿。 */
  const pendingFinal = React.useRef('');
  /**
   * 已写入草稿的句序号。
   *
   * 关闭语音时宿主可能对同一句补一次 flush，服务端也就再发一遍 `final`；
   * 同一句只该落一次，否则用户会看到识别结果被重复追加。
   */
  const lastFinalSeq = React.useRef<number | null>(null);
  /** 语音对话模式的当前轮次；停播回执按它对齐。 */
  const epochRef = React.useRef(0);
  /** 已经等了宿主 `stopAudio` 的轮次；回执只在这个前提下才发。 */
  const drainEpoch = React.useRef(0);
  /**
   * 本会话是否已经收口。
   *
   * 服务端在收口之后仍可能补发 `state(listening)`，若照它把界面拉回聆听，
   * 麦克风按钮就会停在「结束语音输入」而实际并没有在采集，再点一次也关不掉。
   */
  const deadRef = React.useRef(false);
  /** 这一次的识别结果是否作废（用户取消）。 */
  const discardRef = React.useRef(false);
  /** 关闭语音后，等定稿回来就主动收线。 */
  const closeAfterFinalRef = React.useRef(false);
  const textRef = React.useRef(text);
  textRef.current = text;
  const commitRef = React.useRef(commit);
  commitRef.current = commit;
  const onTurnRef = React.useRef(onTurn);
  onTurnRef.current = onTurn;
  const readAloudRef = React.useRef(readAloud);
  readAloudRef.current = readAloud;
  /** 帧处理器经 ref 读采集态：它不该随 active 变化重建事件订阅。 */
  const activeRef = React.useRef(active);
  activeRef.current = active;
  /** 授权确认后要继续的动作；hook 比 `startListening` 先声明，因此经 ref 转发。 */
  const startRef = React.useRef<() => void>(() => undefined);
  const access = useVoicePermission({
    mode,
    onConfirmed: () => startRef.current(),
    onError: setError,
  });

  /** 失败结论按一次性回执处理，到点自己退场。 */
  useExpiringNotice(error, () => setError(null));

  const clearFinalizeTimer = React.useCallback(() => {
    if (finalizeTimer.current === null) return;
    window.clearTimeout(finalizeTimer.current);
    finalizeTimer.current = null;
  }, []);

  const stopElapsed = React.useCallback(() => {
    if (elapsedTimer.current !== null) {
      window.clearInterval(elapsedTimer.current);
      elapsedTimer.current = null;
    }
    startedAt.current = 0;
    setElapsedMs(0);
  }, []);

  /** 到点自动收口的入口经 ref 取，避免计时器把依赖绑成循环。 */
  const autoFinishRef = React.useRef<() => void>(() => undefined);

  const startElapsed = React.useCallback(() => {
    stopElapsed();
    startedAt.current = Date.now();
    setElapsedMs(0);
    elapsedTimer.current = window.setInterval(() => {
      const elapsed = Date.now() - startedAt.current;
      setElapsedMs(elapsed);
      if (elapsed >= MAX_RECORDING_MS) autoFinishRef.current();
    }, ELAPSED_TICK_MS);
  }, [stopElapsed]);

  const stopCapture = React.useCallback(() => {
    captureRef.current?.stop();
    captureRef.current = null;
    resetLevel();
    setActive(false);
  }, []);

  const releasePlayer = React.useCallback(() => {
    playerRef.current?.stop();
    playerRef.current?.destroy();
    playerRef.current = null;
    epochRef.current = 0;
    drainEpoch.current = 0;
  }, []);

  /** 把定稿写进输入坞：只追加，不覆盖用户已经写下的内容。 */
  const writeDraft = React.useCallback((utterance: string) => {
    const next = utterance.trim();
    if (next.length === 0) return;
    commitRef.current(joinDraft(textRef.current, next));
  }, []);

  /**
   * 主动收线：告诉服务端这次会话到此为止。
   *
   * 不主动收线的话，连接会一直挂到服务端空闲超时，然后回一条
   * 「会话长时间无操作」的错误，而那时用户早已关掉语音。
   */
  const closeSession = React.useCallback(() => {
    if (!closeAfterFinalRef.current) return;
    closeAfterFinalRef.current = false;
    void ipc.voiceCancel('client_close').catch(() => undefined);
  }, []);

  /** 安静收口：停采集、停计时、回待机，不留错误回执。 */
  const settle = React.useCallback(
    (nextPhase: VoicePhase) => {
      deadRef.current = true;
      clearFinalizeTimer();
      stopCapture();
      stopElapsed();
      releasePlayer();
      setPartial('');
      setPhase(nextPhase);
    },
    [clearFinalizeTimer, releasePlayer, stopCapture, stopElapsed],
  );

  const fail = React.useCallback(
    (message: string) => {
      generationRef.current += 1;
      settle('idle');
      setError(message);
    },
    [settle],
  );

  /** 停播回执：只有收到过 `stopAudio` 才发，避免与服务端的轮次判定错位。 */
  const acknowledgeDrain = React.useCallback(() => {
    const epoch = drainEpoch.current;
    if (epoch === 0) return;
    drainEpoch.current = 0;
    void ipc.voicePlaybackDrained(epoch).catch(() => undefined);
  }, []);

  const handleFrame = React.useCallback(
    (frame: VoiceServerFrame) => {
      /*
       * 已收口的会话不再接受状态回流，但 `final` 与 `closed` 仍要处理：
       * 前者是关闭后才到达的定稿，后者负责复位入口。
       */
      if (deadRef.current && frame.type !== 'closed' && frame.type !== 'final') return;
      switch (frame.type) {
        case 'ready':
          setPhase((current) => (current === 'idle' ? 'arming' : current));
          break;
        case 'state': {
          const next = phaseFromServerState(frame.phase);
          if (next === 'idle') {
            setPartial('');
            return;
          }
          if (next === 'listening' && !activeRef.current) {
            // 听写收口之后服务端会回到聆听：界面不再停在聆听。
            setPartial('');
            return;
          }
          setPhase(next);
          break;
        }
        case 'partial':
          setPartial(frame.text ?? '');
          setPhase('listening');
          break;
        case 'final': {
          // 用户已经取消：定稿作废，绝不回填输入坞。
          if (discardRef.current) {
            discardRef.current = false;
            deadRef.current = true;
            clearFinalizeTimer();
            settle('idle');
            break;
          }
          const finalText = (frame.text ?? '').trim();
          const seq = typeof frame.utteranceSeq === 'number' ? frame.utteranceSeq : null;
          if (seq !== null && lastFinalSeq.current === seq) break;
          lastFinalSeq.current = seq;
          pendingFinal.current = finalText;
          if (mode === 'dictation') {
            writeDraft(finalText);
            settle('idle');
            closeSession();
            break;
          }
          // 对话模式：等网关的 `turn` 给出回合归属，定稿由网关提交。
          clearFinalizeTimer();
          stopCapture();
          stopElapsed();
          setPartial('');
          setPhase('thinking');
          break;
        }
        case 'turn': {
          if (mode !== 'conversation') break;
          clearFinalizeTimer();
          const interactionId = frame.interactionId ?? '';
          epochRef.current = typeof frame.epoch === 'number' ? frame.epoch : 0;
          pendingFinal.current = '';
          if (interactionId.length > 0) {
            setPhase('thinking');
            if (readAloudRef.current) rememberStreamedTurn(interactionId);
            onTurnRef.current?.(interactionId, '');
          }
          break;
        }
        case 'audio': {
          if (mode !== 'conversation' || frame.pcmBase64 === undefined) break;
          playerRef.current?.push(frame.pcmBase64);
          break;
        }
        case 'stopAudio': {
          if (mode !== 'conversation') break;
          const epoch = typeof frame.epoch === 'number' ? frame.epoch : epochRef.current;
          if (epoch > 0) drainEpoch.current = epoch;
          const player = playerRef.current;
          if (player === null) acknowledgeDrain();
          else player.stop({ drain: true });
          break;
        }
        case 'error': {
          // 背压是内部事实：丢的是最早的音频，用户既听不出也不该被告知。
          if (frame.code === 'backpressure') break;
          // 这一句没听到内容不算故障：听写安静收口，对话保持会话等下一句。
          if (frame.code === 'submit_rejected') {
            if (mode === 'dictation') settle('idle');
            else setPhase('listening');
            break;
          }
          if (mode === 'dictation') writeDraft(pendingFinal.current);
          fail(describeVoiceError(frame.code, frame.message ?? '语音输入失败，请重试'));
          break;
        }
        case 'closed':
          settle('idle');
          break;
        default:
          break;
      }
    },
    [acknowledgeDrain, clearFinalizeTimer, fail, mode, settle, stopCapture, stopElapsed, writeDraft],
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

  /**
   * 关闭语音：停采集并让服务端对当前句定稿。
   *
   * 界面立刻回待机：麦克风已经关了，这一行就不该再写着「思考」。
   * 定稿随后静默写进输入坞；8 秒内没等到就说明这一句没有内容。
   */
  const shutdown = React.useCallback(() => {
    generationRef.current += 1;
    clearFinalizeTimer();
    stopCapture();
    stopElapsed();
    setPartial('');
    setPhase('idle');
    void stopVoiceSession().catch(() => undefined);
    if (mode === 'conversation') {
      void ipc.voiceCancel('user_cancel').catch(() => undefined);
      playerRef.current?.stop({ drain: true });
    }
    /*
     * 关闭之后这一路连接不该继续挂着：服务端会在空闲超时后回一条
     * 「会话长时间无操作」的错误，而用户此时早就关掉了语音。
     * 因此等定稿回来（或超时）就主动收线。
     */
    closeAfterFinalRef.current = true;
    deadRef.current = true;
    finalizeTimer.current = window.setTimeout(() => {
      finalizeTimer.current = null;
      pendingFinal.current = '';
      closeSession();
    }, FINALIZE_TIMEOUT_MS);
  }, [clearFinalizeTimer, mode, stopCapture, stopElapsed]);

  /** 真正开始采集；此时授权已经确认。 */
  const beginCapture = React.useCallback(
    async (workspace: string, destination: string) => {
      const generation = generationRef.current + 1;
      generationRef.current = generation;
      setError(null);
      setPartial('');
      pendingFinal.current = '';
      epochRef.current = 0;
      drainEpoch.current = 0;
      lastFinalSeq.current = null;
      deadRef.current = false;
      discardRef.current = false;
      setPhase('arming');
      setActive(true);
      startElapsed();
      try {
        // 先建立语音会话：宿主拉起与模型预热都在这一步，期间不采集，
        // 因此不会出现「还没连上就开始读麦克风」造成的积压。
        await startVoiceSession(workspace, destination, {
          tts: readAloudRef.current,
          submit: mode === 'conversation',
        });
        if (generationRef.current !== generation) {
          void stopVoiceSession().catch(() => undefined);
          return;
        }
        if (mode === 'conversation') {
          playerRef.current = createVoiceStreamPlayer({
            onDrained: () => acknowledgeDrain(),
            onError: (message) => {
              if (generationRef.current === generation) setError(message);
            },
          });
        }
        const capture = createVoiceCapture(
          {
            onError: (message) => {
              if (generationRef.current === generation) fail(message);
            },
          },
          { deviceId: selectedDeviceId() },
        );
        captureRef.current = capture;
        await capture.start();
        if (generationRef.current !== generation) {
          capture.stop();
          void stopVoiceSession().catch(() => undefined);
          return;
        }
        capture.setForwarding(true);
        setPhase('listening');
        startElapsed();
      } catch (failure) {
        if (generationRef.current !== generation) return;
        captureRef.current?.stop();
        captureRef.current = null;
        setActive(false);
        setPhase('idle');
        stopElapsed();
        const message = describeFailure(failure);
        // 采集被系统拒绝时不要再让用户重复点：直接给系统设置入口。
        if (/权限|NotAllowedError|禁止|拒绝/.test(message)) {
          const blocked = await readMicrophoneAccess().catch(() => null);
          if (blocked !== null && isBlockedBySystem(blocked)) {
            // 系统层拒绝：交给授权引导给出一键打开设置的入口。
            void access.ensureAccess();
            return;
          }
        }
        setError(message);
      }
    },
    [acknowledgeDrain, fail, mode, startElapsed, stopElapsed],
  );

  /** 解析目标会话：首屏没有车道时先建一条。 */
  const resolveTarget = React.useCallback(async (): Promise<VoiceEntry | null> => {
    const destination = replyDestination.trim();
    if (workspaceId !== null && destination.length > 0) {
      return { workspaceId, replyDestination: destination };
    }
    if (ensureSession === undefined) return null;
    return ensureSession();
  }, [ensureSession, replyDestination, workspaceId]);

  /** 入口可用：语音入口可见、桌面宿主，并且有工作区或能在首屏建一条。 */
  const available = React.useMemo(
    () => VOICE_UI_ENABLED && enabled && isDesktopHost() && (workspaceId !== null || ensureSession !== undefined),
    [enabled, ensureSession, workspaceId],
  );

  const startListening = React.useCallback(async () => {
    if (!available) return;
    // 首次说明与系统隐私拦截都在这一步处理；放行才继续。
    if (!(await access.ensureAccess())) return;
    // 先看运行时装好没有：没装就别建立连接再等一次冷启动失败。
    const runtime = await readVoiceRuntime();
    if (runtime !== null && !runtime.available) {
      setError(VOICE_RUNTIME_MISSING_TEXT);
      return;
    }
    const target = await resolveTarget();
    if (target === null) {
      setError('当前会话缺少工作区，无法开始语音输入');
      return;
    }
    await beginCapture(target.workspaceId, target.replyDestination);
  }, [available, beginCapture, mode, resolveTarget, access]);

  // 授权确认后继续的动作：hook 早于这个回调声明，因此在这里补上实现。
  startRef.current = () => void startListening();

  /** 停止采集并等定稿写进输入坞。 */
  const finish = shutdown;

  /**
   * 取消本次语音：丢弃这一句的识别结果。
   *
   * 与「停止」的区别只在结果去留：取消不会把识别文字写进输入坞，
   * 因此误点、或者说到一半发现不想要时，草稿保持原样。
   */
  const cancel = React.useCallback(() => {
    generationRef.current += 1;
    discardRef.current = true;
    // 本地已经收口：服务端随后回的 `state(listening)` 不得把界面拉回录音态。
    deadRef.current = true;
    clearFinalizeTimer();
    stopCapture();
    stopElapsed();
    setPartial('');
    setPhase('idle');
    void ipc.voiceCancel('user_cancel').catch(() => undefined);
    void stopVoiceSession().catch(() => undefined);
    closeSession();
  }, [clearFinalizeTimer, closeSession, stopCapture, stopElapsed]);

  const toggle = React.useCallback(() => {
    if (access.prompt !== null) {
      access.dismiss();
      return;
    }
    // 开着的语音一律先关：按钮是开关，关闭即定稿。
    if (active || phase !== 'idle') {
      finish();
      return;
    }
    void startListening();
  }, [access, active, finish, phase, startListening]);

  /*
   * Esc 关闭语音：这是「停下」的最短路径，不必把指针移回麦克风。
   *
   * 用捕获阶段并在命中时立刻截停：语音开着的这一下 Esc 只属于语音，
   * 不该顺手把正在进行的回复也中断掉。等语音关掉之后，下一次 Esc 才回到
   * 「中断本轮」的语义。
   */
  React.useEffect(() => {
    if (!active && phase === 'idle') return undefined;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.isComposing || event.repeat) return;
      if (event.defaultPrevented) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      shutdown();
    };
    window.addEventListener('keydown', handleKeyDown, { capture: true });
    return () => window.removeEventListener('keydown', handleKeyDown, { capture: true });
  }, [active, phase, shutdown]);

  // 到点自动停止：先定稿写入草稿，再给一条一次性说明。
  autoFinishRef.current = () => {
    if (!activeRef.current) return;
    shutdown();
    setError('录音已达 120 秒上限，已自动停止并识别');
  };

  React.useEffect(() => {
    if (available || !active) return;
    shutdown();
  }, [active, available, shutdown]);

  React.useEffect(
    () => () => {
      generationRef.current += 1;
      if (finalizeTimer.current !== null) window.clearTimeout(finalizeTimer.current);
      if (elapsedTimer.current !== null) window.clearInterval(elapsedTimer.current);
      captureRef.current?.stop();
      captureRef.current = null;
      playerRef.current?.destroy();
      playerRef.current = null;
      void stopVoiceSession().catch(() => undefined);
      void ipc.voiceCancel('client_close').catch(() => undefined);
      resetLevel();
    },
    [],
  );

  return {
    available,
    phase,
    partial,
    error,
    active,
    elapsedMs,
    permission: access.prompt,
    toggle,
    finish,
    cancel,
    confirmPermission: access.confirm,
    dismissPermission: access.dismiss,
    openSettings: access.openSettings,
    retryPermission: access.retry,
  };
}