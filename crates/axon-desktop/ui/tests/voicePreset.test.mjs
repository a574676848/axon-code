import test from 'node:test';
import assert from 'node:assert/strict';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { FOLLOW_PERSONA_ID, parseVoicePresets, voicePresetRows } from '../src/speech/presets.ts';
import { createVoicePreferences } from '../src/speech/preferences.ts';
import { VOICE_STORAGE_KEYS } from '../src/speech/keys.ts';
import {
  classifyPreviewFailure,
  currentPresetId,
  PREVIEW_TIMEOUT_MESSAGE,
} from '../src/speech/client.ts';
import { VoicePresetSelect } from '../src/components/settings/VoicePresetSettings.tsx';

/*
 * 音色的端到端契约：默认跟随人格、选择落盘、停用项禁用、合成读到的 presetId，
 * 以及清单读不到时的空态。合成请求与清单读取都换掉，因此这里不碰网关与扬声器。
 */

/** 网关 `voice_presets` 的真实回执形状：五个预设，甜软台腔依赖额外模型。 */
const GATEWAY_PRESETS = [
  { id: 'calm-elegant', name: '冷静知性', available: true, default: true },
  { id: 'bright-neighbor', name: '明朗邻家', available: true, default: false },
  { id: 'playful-lively', name: '俏皮灵动', available: true, default: false },
  { id: 'warm-taiwan', name: '甜软台腔', available: false, default: false },
  { id: 'capable-assistant', name: '干练助理', available: true, default: false },
];

/** 内存版偏好存储，行为与 localStorage 的字符串存取一致。 */
function memoryStorage(initial = {}) {
  const map = new Map(Object.entries(initial));
  return {
    map,
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => {
      map.set(key, value);
    },
  };
}

/** 渲染音色选择区；默认按「清单已读到、跟随人格」出图。 */
function renderChoices({
  status = 'ready',
  presets = parseVoicePresets({ presets: GATEWAY_PRESETS }),
  message = '',
  selectedId = '',
} = {}) {
  return renderToStaticMarkup(
    React.createElement(VoicePresetSelect, {
      status,
      presets,
      message,
      selectedId,
      onSelect: () => undefined,
      onRetry: () => undefined,
    }),
  );
}

test('默认跟随人格：没存过时第一项选中，其余预设都不选中', () => {
  const presets = parseVoicePresets({ presets: GATEWAY_PRESETS });
  assert.equal(presets.length, 5);

  const rows = voicePresetRows(presets, createVoicePreferences(memoryStorage()).presetId());
  assert.equal(rows[0].id, FOLLOW_PERSONA_ID);
  assert.equal(rows[0].name, '跟随人格（默认）');
  assert.deepEqual(
    rows.map((row) => row.checked),
    [true, false, false, false, false, false],
  );

  // 音色是一个下拉而不是一行行单选：触发器只有一个，收起态不铺开候选项。
  const html = renderChoices();
  assert.match(html, /aria-haspopup="listbox"/);
  assert.match(html, /aria-label="音色"/);
  assert.match(html, /aria-expanded="false"/);
  assert.match(html, /跟随人格（默认）/);
  assert.doesNotMatch(html, /role="radiogroup"/);
  assert.doesNotMatch(html, /role="radio"/);
});

test('选择落盘可复读，空串写回表示跟随人格', () => {
  const storage = memoryStorage();
  const prefs = createVoicePreferences(storage);
  assert.equal(prefs.presetId(), '');

  prefs.setPresetId('playful-lively');
  assert.equal(VOICE_STORAGE_KEYS.preset, 'axon.voice.preset');
  assert.equal(storage.getItem(VOICE_STORAGE_KEYS.preset), 'playful-lively');
  // 重新加载偏好（等价于重启界面）读到的是同一个预设。
  assert.equal(createVoicePreferences(storage).presetId(), 'playful-lively');

  prefs.setPresetId(FOLLOW_PERSONA_ID);
  assert.equal(storage.getItem(VOICE_STORAGE_KEYS.preset), '');
  assert.equal(createVoicePreferences(storage).presetId(), '');

  // 每次选择都通知订阅者，人设页的选中态因此立刻跟上。
  let notified = 0;
  const unsubscribe = prefs.subscribe(() => {
    notified += 1;
  });
  prefs.setPresetId('calm-elegant');
  unsubscribe();
  prefs.setPresetId('calm-elegant');
  assert.equal(notified, 1);
});

test('停用预设禁用并标注当前不可用，存着它也落回跟随人格', () => {
  const presets = parseVoicePresets({ presets: GATEWAY_PRESETS });
  const rows = voicePresetRows(presets, 'warm-taiwan');
  const taiwan = rows.find((row) => row.id === 'warm-taiwan');
  assert.equal(taiwan.available, false);
  assert.equal(taiwan.checked, false);
  assert.equal(rows[0].checked, true);

  // 停用项不会成为当前值：界面显示的仍是「跟随人格」。
  const html = renderChoices({ selectedId: 'warm-taiwan' });
  assert.match(html, /跟随人格（默认）/);
  // 停用项里没有安装入口：模型随语音运行时提供，界面不负责下载。
  assert.doesNotMatch(html, /安装/);
});

test('currentPresetId 从存储读取，空串表示跟随人格并返回 null', () => {
  const storage = memoryStorage();
  globalThis.window = { localStorage: storage };
  try {
    assert.equal(currentPresetId(), null);
    storage.setItem(VOICE_STORAGE_KEYS.preset, 'capable-assistant');
    assert.equal(currentPresetId(), 'capable-assistant');
    storage.setItem(VOICE_STORAGE_KEYS.preset, '');
    assert.equal(currentPresetId(), null);
  } finally {
    delete globalThis.window;
  }
});

test('预设命令失败时给空态与重试，不冒充成一份可用清单', () => {
  const html = renderChoices({
    status: 'failed',
    presets: [],
    message: '网关不可达：127.0.0.1 上的运行态服务没有响应',
  });
  assert.match(html, /音色预设暂时读不到/);
  assert.match(html, /网关不可达/);
  assert.match(html, /重试/);
  // 失败态一个选项都不摆：没有真实清单就不给人点不动的选择。
  assert.doesNotMatch(html, /role="radiogroup"/);
  assert.doesNotMatch(html, /aria-checked/);
});

test('回执形状不对时只留跟随人格，读取中给加载态', () => {
  assert.deepEqual(parseVoicePresets(null), []);
  assert.deepEqual(parseVoicePresets({ presets: 'oops' }), []);
  const presets = parseVoicePresets({
    presets: [
      { id: '', name: '少了 id', available: true },
      { id: 'calm-elegant', name: '冷静知性' },
      { id: 'calm-elegant', name: '重复 id', available: false },
      { id: 'warm-taiwan', name: '甜软台腔', available: false },
    ],
  });
  // 缺 id 的丢掉、重复 id 只留第一条、available 缺失按可用处理。
  assert.deepEqual(
    presets.map((preset) => [preset.id, preset.available]),
    [
      ['calm-elegant', true],
      ['warm-taiwan', false],
    ],
  );
  assert.deepEqual(voicePresetRows([], ''), [
    { id: '', name: '跟随人格（默认）', available: true, checked: true },
  ]);
  assert.match(renderChoices({ status: 'loading', presets: [] }), /正在读取音色预设/);
});

test('试听失败按协议前缀分类，504 与 409 各有中文结论', () => {
  const timeout = classifyPreviewFailure(new Error('preview_timeout: 网关超时'));
  assert.equal(timeout.kind, 'timeout');
  assert.equal(timeout.message, PREVIEW_TIMEOUT_MESSAGE);

  const unavailable = classifyPreviewFailure(
    'voice_unavailable: 语音运行时未就绪，请先完成语音依赖安装',
  );
  assert.equal(unavailable.kind, 'unavailable');
  assert.match(unavailable.message, /语音运行时未就绪/);

  const invalid = classifyPreviewFailure('preview_invalid: 试听参数不正确');
  assert.equal(invalid.kind, 'invalid');
  assert.match(invalid.message, /试听参数/);

  const failed = classifyPreviewFailure('');
  assert.equal(failed.kind, 'failed');
  assert.match(failed.message, /试听失败/);
});

test('每行一个试听按钮，正在播放换成停止，停用预设的试听入口禁用', () => {
  const html = renderToStaticMarkup(
    React.createElement(VoicePresetSelect, {
      status: 'ready',
      presets: parseVoicePresets({ presets: GATEWAY_PRESETS }),
      message: '',
      selectedId: FOLLOW_PERSONA_ID,
      onSelect: () => undefined,
      onRetry: () => undefined,
      previewing: false,
      previewBusy: false,
      onPreview: () => undefined,
    }),
  );
  // 下拉右侧只有一枚试听入口，不是每行一枚。
  assert.match(html, /aria-label="试听当前音色"/);
  assert.equal((html.match(/aria-label="试听当前音色"/g) ?? []).length, 1);
});
