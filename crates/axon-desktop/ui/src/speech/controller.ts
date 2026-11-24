import { speakableScript } from './sanitize';
import { splitSpeechSegments } from './segments';

/*
 * 朗读控制器：同一时刻只允许一条朗读。
 *
 * 播放器与合成请求都由外部注入，因此这一层只承担状态机：
 * 开始新的一条先停旧的，任何一条收口（念完、播放失败、被顶掉、被停止）
 * 都会把状态清回静息并回调调用方。真正的音频与网关请求分别在
 * audioPlayer.ts 与 client.ts，测试可以换成内存实现。
 *
 * 长文按句分段逐段合成：整段一次送进本地合成既慢又容易超时，而朗读本来就是
 * 一串句子。第一段合成完成即开始发声，同时预取下一段，段与段之间不留空拍。
 */

/** 合成侧的入参上限，与网关契约一致；超出部分直接截断，不把必然被拒的请求发出去。 */
export const MAX_SPEECH_CHARS = 4000;

/** 语音运行时未就绪后的重试窗口：过了这段时间按钮恢复可点，不必重启界面。 */
const UNAVAILABLE_RETRY_MS = 12000;

export type SpeechStatus = 'idle' | 'preparing' | 'playing' | 'unavailable' | 'error';

export interface SpeechState {
  status: SpeechStatus;
  /** 发起这一条朗读的消息标识；静息与不可用态为空。 */
  source: string | null;
  /** 不可用或失败时的一句话，直接作为按钮提示。 */
  message: string | null;
}

/** 网关返回的一段音频。 */
export interface VoiceAudioPayload {
  audioBase64: string;
  mimeType: string;
}

export type VoiceFailureKind = 'unavailable' | 'blocked' | 'failed';

/** 合成失败：kind 决定界面怎么提示，message 直接展示给用户。 */
export class VoiceFailure extends Error {
  readonly kind: VoiceFailureKind;

  constructor(kind: VoiceFailureKind, message: string) {
    super(message);
    this.name = 'VoiceFailure';
    this.kind = kind;
  }
}

/** 播放器回调：念完与播放失败各一次。 */
export interface PlayerEvents {
  onEnded: () => void;
  onFailed: (failure: unknown) => void;
}

/** 播放器句柄：停止必须同时暂停与释放音频地址。 */
export interface VoicePlayerHandle {
  play(): Promise<void>;
  stop(): void;
}

export interface SpeakOptions {
  /** 发起方标识，用于把「正在念的是哪一条」回给按钮。 */
  source?: string;
  /** 这一条收口时回调，不论念完还是被停止。 */
  onEnd?: () => void;
}

export interface SpeechControllerOptions {
  requestAudio: (text: string, presetId: string | null) => Promise<VoiceAudioPayload>;
  createPlayer: (payload: VoiceAudioPayload, events: PlayerEvents) => VoicePlayerHandle;
  /** 当前音色预设；没有音色选择时返回 null，由网关使用默认音色。 */
  presetId?: () => string | null;
  /** 延时器；默认用 setTimeout，测试可以换成手动推进。 */
  schedule?: (delayMs: number, run: () => void) => () => void;
}

export interface SpeechController {
  speak(text: string, options?: SpeakOptions): void;
  stop(): void;
  state(): SpeechState;
  subscribe(listener: () => void): () => void;
}

const IDLE: SpeechState = { status: 'idle', source: null, message: null };

/** 默认延时器：到点执行一次。 */
function scheduleTimer(delayMs: number, run: () => void): () => void {
  const timer = setTimeout(run, delayMs);
  return () => clearTimeout(timer);
}

/** 把任意失败归一成一句话。 */
function messageOf(failure: unknown): string {
  if (failure instanceof Error) return failure.message;
  return typeof failure === 'string' && failure.length > 0 ? failure : '朗读失败';
}

export function createSpeechController(options: SpeechControllerOptions): SpeechController {
  const listeners = new Set<() => void>();
  const schedule = options.schedule ?? scheduleTimer;
  let stateValue: SpeechState = IDLE;
  let sequence = 0;
  let current: {
    id: number;
    source: string | null;
    onEnd?: () => void;
    player: VoicePlayerHandle | null;
    segments: string[];
    next: number;
    /** 已预取但还没播的下一段。 */
    ahead: Promise<VoiceAudioPayload> | null;
    presetId: string | null;
  } | null = null;
  /** 不可用态的恢复计时器；不为空时下一次到点恢复可点。 */
  let cancelRetry: (() => void) | null = null;

  const notify = (): void => {
    for (const listener of listeners) listener();
  };

  const setState = (next: SpeechState): void => {
    stateValue = next;
    notify();
  };

  const clearRetry = (): void => {
    if (cancelRetry === null) return;
    cancelRetry();
    cancelRetry = null;
  };

  /** 一条朗读收口：释放播放器、落状态、回调调用方。 */
  const finish = (id: number, next: SpeechState): void => {
    if (current === null || current.id !== id) return;
    const done = current;
    current = null;
    done.player?.stop();
    setState(next);
    done.onEnd?.();
  };

  const stop = (): void => {
    if (current === null) return;
    finish(current.id, IDLE);
  };

  const markUnavailable = (source: string | null, message: string): void => {
    clearRetry();
    cancelRetry = schedule(UNAVAILABLE_RETRY_MS, () => {
      cancelRetry = null;
      if (stateValue.status === 'unavailable') setState(IDLE);
    });
    setState({ status: 'unavailable', source, message });
  };

  const speak = (text: string, speakOptions: SpeakOptions = {}): void => {
    const script = speakableScript(text).slice(0, MAX_SPEECH_CHARS);
    const segments = splitSpeechSegments(script);
    if (segments.length === 0) return;
    // 单实例：新的朗读先把旧的停掉，旧的一条按「被顶掉」收口。
    clearRetry();
    stop();
    const id = ++sequence;
    const source = speakOptions.source ?? null;
    const presetId = options.presetId?.() ?? null;
    current = {
      id,
      source,
      onEnd: speakOptions.onEnd,
      player: null,
      segments,
      next: 0,
      ahead: null,
      presetId,
    };
    setState({ status: 'preparing', source, message: null });

    const fail = (failure: unknown): void => {
      if (current === null || current.id !== id) return;
      const voiceFailure =
        failure instanceof VoiceFailure ? failure : new VoiceFailure('failed', messageOf(failure));
      const ended = current.onEnd;
      if (voiceFailure.kind === 'unavailable') {
        // 运行时未就绪不是坏了：落一个可恢复的不可用态，过一会自动放开重试。
        current = null;
        markUnavailable(source, voiceFailure.message);
        ended?.();
        return;
      }
      finish(id, { status: 'error', source, message: voiceFailure.message });
    };

    /** 取第 index 段音频：优先用已经预取的那一份。 */
    const requestAt = (index: number, preset: string | null): Promise<VoiceAudioPayload> => {
      if (index === current?.next && current.ahead !== null) {
        const ahead = current.ahead;
        current.ahead = null;
        return ahead;
      }
      return options.requestAudio(segments[index], preset);
    };

    /** 预取下一段；失败留给正片那一段处理，不在这里抢答。 */
    const prefetchNext = (from: number, preset: string | null): void => {
      const target = from + 1;
      if (current === null || current.id !== id) return;
      if (target >= segments.length || current.ahead !== null || target <= current.next) return;
      current.ahead = options.requestAudio(segments[target], preset);
      // 预取失败不该成为未处理的拒绝：真正的请求会在轮到它时重新发起。
      current.ahead.catch(() => undefined);
    };

    const playAt = (index: number): void => {
      if (current === null || current.id !== id) return;
      if (index >= segments.length) {
        finish(id, IDLE);
        return;
      }
      let request: Promise<VoiceAudioPayload>;
      try {
        request = requestAt(index, presetId);
      } catch (failure) {
        fail(failure);
        return;
      }
      request.then((payload) => {
        // 请求在途时被停止或顶掉：不再发声，也不留下音频地址。
        if (current === null || current.id !== id) return;
        const started = current;
        started.next = index + 1;
        let player: VoicePlayerHandle;
        try {
          player = options.createPlayer(payload, {
            onEnded: () => playAt(index + 1),
            onFailed: (failure: unknown) => fail(failure),
          });
        } catch (failure) {
          fail(failure);
          return;
        }
        started.player = player;
        if (stateValue.status !== 'playing') {
          setState({ status: 'playing', source, message: null });
        }
        // 下一段在发声的同时开始合成，段与段之间不留空拍。
        prefetchNext(index, presetId);
        player.play().catch((failure: unknown) => {
          fail(
            new VoiceFailure(
              'blocked',
              failure instanceof Error && failure.name === 'NotAllowedError'
                ? '浏览器阻止了自动播放，点喇叭重试'
                : messageOf(failure),
            ),
          );
        });
      }, fail);
    };

    playAt(0);
  };

  return {
    speak,
    stop,
    state: () => stateValue,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}
