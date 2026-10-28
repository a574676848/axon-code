import { publishLevel, resetLevel } from './levelBus';

/*
 * 麦克风输入设备的选择与实时电平预览。
 *
 * 与成熟语音输入产品一致：设备是用户自己的选择，选过就记住；明确指定的设备
 * 不会静默回退到别的输入（否则「我选了耳机麦却在用桌面麦」根本看不出来）。
 * 预览只用 Web Audio 读电平，不录音、不落盘。
 */

/** 输入设备偏好键。 */
const DEVICE_KEY = 'axon.voice.input-device';

/** 跟随系统默认设备。 */
export const SYSTEM_DEFAULT_DEVICE = '';

/** 一个可选输入设备。 */
export interface InputDevice {
  /** `deviceId`；空串表示系统默认。 */
  id: string;
  /** 面向用户的名称；未授权时浏览器只给空串，这里补一句可读的占位。 */
  label: string;
}

/** 读取已保存的设备选择；空串表示跟随系统。 */
export function selectedDeviceId(): string {
  if (typeof window === 'undefined') return SYSTEM_DEFAULT_DEVICE;
  try {
    return window.localStorage.getItem(DEVICE_KEY) ?? SYSTEM_DEFAULT_DEVICE;
  } catch {
    return SYSTEM_DEFAULT_DEVICE;
  }
}

/** 保存设备选择。 */
export function setSelectedDeviceId(deviceId: string): void {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(DEVICE_KEY, deviceId);
  } catch {
    // 写不进去只影响下次启动的记住状态，本次照常生效。
  }
}

/**
 * 列出可用的音频输入设备。
 *
 * 未授权时浏览器会把 `label` 擦掉，这里给一个按序号的占位名，让用户至少能
 * 区分「哪个是第一路输入」；授权后名称会自动补齐。
 */
export async function listInputDevices(): Promise<InputDevice[]> {
  const devices: InputDevice[] = [{ id: SYSTEM_DEFAULT_DEVICE, label: '系统默认麦克风' }];
  if (typeof navigator === 'undefined' || navigator.mediaDevices?.enumerateDevices === undefined) {
    return devices;
  }
  try {
    const all = await navigator.mediaDevices.enumerateDevices();
    let index = 0;
    for (const device of all) {
      if (device.kind !== 'audioinput') continue;
      index += 1;
      if (device.deviceId === 'default' || device.deviceId === '') continue;
      devices.push({
        id: device.deviceId,
        label: device.label.trim().length > 0 ? device.label : `麦克风 ${index}`,
      });
    }
  } catch {
    // 枚举失败时只留系统默认，界面不编造候选。
  }
  return devices;
}

/** 一个可启停的电平预览。 */
export interface LevelPreview {
  /** 打开指定设备并持续上报电平；`deviceId` 为空表示系统默认。 */
  start(deviceId: string): Promise<void>;
  /** 停止预览并释放设备。 */
  stop(): void;
}

/** 预览的分析窗口：512 样本约 11 ms，足够跟上说话起伏。 */
const PREVIEW_FFT = 512;

/** 创建电平预览；同一实例可反复 start 与 stop。 */
export function createLevelPreview(): LevelPreview {
  let stream: MediaStream | null = null;
  let context: AudioContext | null = null;
  let analyser: AnalyserNode | null = null;
  let frame = 0;
  let running = false;

  const release = (): void => {
    running = false;
    if (frame !== 0) {
      window.cancelAnimationFrame(frame);
      frame = 0;
    }
    analyser?.disconnect();
    analyser = null;
    if (context !== null) {
      void context.close().catch(() => undefined);
      context = null;
    }
    if (stream !== null) {
      for (const track of stream.getTracks()) track.stop();
      stream = null;
    }
    resetLevel();
  };

  const sample = (): void => {
    if (!running || analyser === null) return;
    frame = window.requestAnimationFrame(sample);
    const data = new Uint8Array(analyser.fftSize);
    analyser.getByteTimeDomainData(data as Uint8Array<ArrayBuffer>);
    let sum = 0;
    for (let index = 0; index < data.length; index += 1) {
      const deviation = (data[index] - 128) / 128;
      sum += deviation * deviation;
    }
    publishLevel(Math.sqrt(sum / data.length) * 3.4);
  };

  const start = async (deviceId: string): Promise<void> => {
    release();
    if (typeof navigator === 'undefined' || navigator.mediaDevices?.getUserMedia === undefined) {
      throw new Error('当前环境不支持麦克风预览');
    }
    const constraints: MediaStreamConstraints = {
      audio: {
        channelCount: 1,
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
        // 明确指定的设备不回退：选的是哪一路就用哪一路。
        ...(deviceId.length > 0 ? { deviceId: { exact: deviceId } } : {}),
      },
    };
    stream = await navigator.mediaDevices.getUserMedia(constraints);
    context = new AudioContext();
    if (context.state === 'suspended') await context.resume().catch(() => undefined);
    const source = context.createMediaStreamSource(stream);
    analyser = context.createAnalyser();
    analyser.fftSize = PREVIEW_FFT;
    source.connect(analyser);
    running = true;
    frame = window.requestAnimationFrame(sample);
  };

  return {
    start,
    stop: () => {
      release();
    },
  };
}
