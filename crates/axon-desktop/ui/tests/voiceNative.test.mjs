import test from 'node:test';
import assert from 'node:assert/strict';
import {
  levelOfPcm16,
  levelOfSamples,
  normalizeLevel,
  publishLevel,
  resetLevel,
  subscribeLevel,
} from '../src/speech/levelBus.ts';
import {
  isBlockedBySystem,
  describeAccessBlock,
  hasMicrophoneConsent,
  rememberMicrophoneConsent,
} from '../src/speech/voicePermission.ts';
import { isBusyPhase, isRecoverableVoiceError } from '../src/speech/voiceProtocol.ts';

/*
 * 原生麦克风授权与声学电平的纯逻辑：系统结论的分类、电平归一化、
 * 以及协议里「可恢复错误」与「机器在忙」的口径。
 * 这里不碰音频设备、系统注册表与 WebView，因此无头环境也能覆盖。
 */

test('s16le 字节的振幅按 RMS 归一，静音为零、满幅接近上限', () => {
  assert.equal(levelOfPcm16(new Uint8Array(0)), 0);
  assert.equal(levelOfPcm16(new Uint8Array([0, 0, 0, 0])), 0);

  const loud = new Uint8Array(200);
  for (let index = 0; index < loud.length; index += 2) {
    loud[index] = 0x00;
    loud[index + 1] = 0x40; // 0x4000 = 16384，约半幅
  }
  const loudLevel = levelOfPcm16(loud);
  assert.ok(loudLevel > 0.5 && loudLevel <= 1, `半幅应映射到高位，实际 ${loudLevel}`);
});

test('浮点采样振幅与归一化同样收敛到 0..1', () => {
  assert.equal(levelOfSamples(new Float32Array(0)), 0);
  assert.equal(levelOfSamples(new Float32Array([0, 0, 0])), 0);
  const level = levelOfSamples(new Float32Array([0.5, -0.5, 0.5, -0.5]));
  assert.ok(level > 0.6 && level <= 1);
  assert.equal(normalizeLevel(-1), 0);
  assert.equal(normalizeLevel(Number.NaN), 0);
  assert.equal(normalizeLevel(3), 1);
});

test('电平总线只在值变化时广播，归零后订阅者能收到 0', () => {
  const seen = [];
  const unsubscribe = subscribeLevel((value) => seen.push(value));
  publishLevel(0.5);
  publishLevel(0.5);
  publishLevel(0.8);
  resetLevel();
  unsubscribe();
  publishLevel(0.9);
  assert.deepEqual(seen, [0.5, 0.8, 0]);
});

test('系统拒绝分类：只有系统与桌面两条拒绝需要去设置里放开', () => {
  assert.equal(isBlockedBySystem({ state: 'system_denied', canOpenSettings: true, settingsLabel: '' }), true);
  assert.equal(isBlockedBySystem({ state: 'desktop_denied', canOpenSettings: true, settingsLabel: '' }), true);
  assert.equal(isBlockedBySystem({ state: 'granted', canOpenSettings: true, settingsLabel: '' }), false);
  assert.equal(isBlockedBySystem({ state: 'unknown', canOpenSettings: true, settingsLabel: '' }), false);
});

test('系统拒绝的文案说清是哪一层拦截', () => {
  assert.match(
    describeAccessBlock({ state: 'system_denied', canOpenSettings: true, settingsLabel: '' }),
    /总开关/,
  );
  assert.match(
    describeAccessBlock({ state: 'desktop_denied', canOpenSettings: true, settingsLabel: '' }),
    /桌面应用/,
  );
  assert.match(
    describeAccessBlock({ state: 'unknown', canOpenSettings: true, settingsLabel: '' }),
    /权限/,
  );
});

test('首次授权说明只记一次，存储不可用时退回未确认', () => {
  const map = new Map();
  globalThis.window = {
    localStorage: {
      getItem: (key) => (map.has(key) ? map.get(key) : null),
      setItem: (key, value) => map.set(key, value),
    },
  };
  try {
    assert.equal(hasMicrophoneConsent(), false);
    rememberMicrophoneConsent();
    assert.equal(hasMicrophoneConsent(), true);
  } finally {
    delete globalThis.window;
  }
  // 没有 window（调试桥外的纯 node 测试）时按未确认处理，不抛异常。
  assert.equal(hasMicrophoneConsent(), false);
});

test('背压是可恢复错误，不让入口复位；忙态覆盖准备、思考与播报', () => {
  assert.equal(isRecoverableVoiceError('backpressure'), true);
  assert.equal(isRecoverableVoiceError('submit_rejected'), false);
  assert.equal(isRecoverableVoiceError(undefined), false);

  assert.equal(isBusyPhase('arming'), true);
  assert.equal(isBusyPhase('thinking'), true);
  assert.equal(isBusyPhase('speaking'), true);
  assert.equal(isBusyPhase('listening'), false);
  assert.equal(isBusyPhase('idle'), false);
});
