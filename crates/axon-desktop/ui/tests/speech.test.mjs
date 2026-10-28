import test from 'node:test';
import assert from 'node:assert/strict';
import { speakableScript } from '../src/speech/sanitize.ts';
import { createVoicePreferences } from '../src/speech/preferences.ts';
import { VOICE_STORAGE_KEYS } from '../src/speech/keys.ts';
import { SEGMENT_MAX_CHARS, splitSpeechSegments } from '../src/speech/segments.ts';
import {
  MAX_SPEECH_CHARS,
  VoiceFailure,
  createSpeechController,
} from '../src/speech/controller.ts';

/*
 * 语音播报的契约：偏好默认关闭且按会话隔离、口播稿净化、以及同一时刻只念一条。
 * 合成请求与音频播放都以内存实现注入，因此这些用例不碰网络与扬声器。
 */

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

/** 把已经解析的 Promise 链推完。 */
function flush(times = 4) {
  let chain = Promise.resolve();
  for (let index = 0; index < times; index += 1) chain = chain.then(() => undefined);
  return chain;
}

test('语音播报默认关闭：没存过时全局与会话都在关闭态', () => {
  const prefs = createVoicePreferences(memoryStorage());
  assert.equal(prefs.globalAutoRead(), false);
  assert.equal(prefs.laneAutoRead('ws1:lane1'), null);
  assert.equal(prefs.effectiveAutoRead('ws1:lane1'), false);
  // 存储不可用时退化为默认值，界面不会被存储异常带崩。
  const withoutStorage = createVoicePreferences(null);
  assert.equal(withoutStorage.globalAutoRead(), false);
  assert.equal(withoutStorage.effectiveAutoRead('ws1:lane1'), false);
});

test('全局开关落盘可复读，未设置过的会话继承全局值', () => {
  const storage = memoryStorage();
  createVoicePreferences(storage).setGlobalAutoRead(true);
  assert.equal(storage.getItem(VOICE_STORAGE_KEYS.global), 'true');
  const reloaded = createVoicePreferences(storage);
  assert.equal(reloaded.globalAutoRead(), true);
  assert.equal(reloaded.effectiveAutoRead('ws1:lane1'), true);
});

test('会话开关只作用于自己：别的会话与全局值都不受影响', () => {
  const storage = memoryStorage();
  const prefs = createVoicePreferences(storage);
  prefs.setLaneAutoRead('ws1:lane1', true);
  assert.equal(prefs.effectiveAutoRead('ws1:lane1'), true);
  assert.equal(prefs.effectiveAutoRead('ws1:lane2'), false);
  assert.equal(prefs.effectiveAutoRead('ws2:lane1'), false);
  assert.equal(prefs.globalAutoRead(), false);
  assert.notEqual(VOICE_STORAGE_KEYS.lane('ws1:lane1'), VOICE_STORAGE_KEYS.lane('ws1:lane2'));

  // 会话显式关闭优先于全局打开，因此「全局打开 + 本条关闭」不会漏出声音。
  prefs.setGlobalAutoRead(true);
  prefs.setLaneAutoRead('ws1:lane1', false);
  assert.equal(prefs.effectiveAutoRead('ws1:lane1'), false);
  assert.equal(prefs.effectiveAutoRead('ws1:lane2'), true);
});

test('开关变化会通知订阅者', () => {
  const prefs = createVoicePreferences(memoryStorage());
  let notified = 0;
  const unsubscribe = prefs.subscribe(() => {
    notified += 1;
  });
  prefs.setGlobalAutoRead(true);
  prefs.setLaneAutoRead('ws1:lane1', true);
  assert.equal(notified, 2);
  unsubscribe();
  prefs.setGlobalAutoRead(false);
  assert.equal(notified, 2);
});

test('口播稿剥离代码：围栏整段不出声，行内反引号只摘标记', () => {
  assert.equal(
    speakableScript('第一段\n\n```ts\nconst a = 1;\n```\n\n第二段'),
    '第一段 第二段',
  );
  assert.equal(speakableScript('先 `npm run build` 再提交'), '先 npm run build 再提交');
  assert.equal(speakableScript('```\n只有代码\n```'), '');
});

test('口播稿剥离 Markdown 标记', () => {
  assert.equal(speakableScript('# 标题\n\n**加粗**与*斜体*'), '标题 加粗与斜体');
  assert.equal(speakableScript('- 第一项\n- 第二项'), '第一项 第二项');
  assert.equal(speakableScript('> 引用的一句话'), '引用的一句话');
  assert.equal(speakableScript('| 项 | 值 |\n| --- | --- |\n| 甲 | 1 |'), '项 值 甲 1');
  assert.equal(speakableScript('[文档](https://example.com/a) 已更新'), '文档 已更新');
});

test('口播稿把绝对路径收敛成文件名，把网址收敛成域名', () => {
  assert.equal(
    speakableScript('见 E:\\zbg\\axon\\crates\\main.rs 这个文件'),
    '见 main.rs 这个文件',
  );
  assert.equal(speakableScript('打开 /home/axon/notes.md 查看'), '打开 notes.md 查看');
  assert.equal(
    speakableScript('参考 https://www.example.com/docs/a?b=1 这一页'),
    '参考 example.com 这一页',
  );
});

test('口播稿把令牌与邮箱替换成类别词', () => {
  assert.equal(speakableScript('密钥 sk-live-abcdef123456 不要外传'), '密钥 令牌 不要外传');
  assert.equal(speakableScript('联系 dev@example.com 处理'), '联系 邮箱 处理');
  assert.equal(speakableScript('凭据 Bearer abcdef123456 过期'), '凭据 令牌 过期');
});

test('同一时刻只念一条：新朗读先停旧的，旧的一条按被顶掉收口', async () => {
  const stopped = [];
  const requests = [];
  const controller = createSpeechController({
    requestAudio: (text) => {
      requests.push(text);
      return Promise.resolve({ audioBase64: 'AA==', mimeType: 'audio/wav' });
    },
    createPlayer: () => ({
      play: () => Promise.resolve(),
      stop: () => {
        stopped.push('stopped');
      },
    }),
    schedule: () => () => undefined,
  });

  let ended = 0;
  controller.speak('**第一条**回复', { source: 'm1', onEnd: () => (ended += 1) });
  await flush();
  assert.equal(controller.state().status, 'playing');
  assert.equal(controller.state().source, 'm1');
  // 送出去的是净化后的口播稿，不是 Markdown 原文。
  assert.equal(requests[0], '第一条回复');

  controller.speak('第二条回复', { source: 'm2' });
  await flush();
  assert.equal(ended, 1);
  assert.equal(stopped.length, 1);
  assert.equal(controller.state().source, 'm2');

  controller.stop();
  assert.equal(controller.state().status, 'idle');
  assert.equal(stopped.length, 2);
});

test('停止后返回的合成结果不再发声，也不会创建播放器', async () => {
  let release;
  let players = 0;
  const controller = createSpeechController({
    requestAudio: () =>
      new Promise((resolve) => {
        release = resolve;
      }),
    createPlayer: () => {
      players += 1;
      return { play: () => Promise.resolve(), stop: () => undefined };
    },
    schedule: () => () => undefined,
  });

  controller.speak('迟到的结果', { source: 'late' });
  controller.stop();
  release({ audioBase64: 'AA==', mimeType: 'audio/wav' });
  await flush();
  assert.equal(players, 0);
  assert.equal(controller.state().status, 'idle');
});

test('语音运行时未就绪：落不可用态并给出一句话，过一窗自动恢复可点', async () => {
  let restore = null;
  let players = 0;
  let ended = 0;
  const controller = createSpeechController({
    requestAudio: () => Promise.reject(new VoiceFailure('unavailable', '语音运行时未就绪')),
    createPlayer: () => {
      players += 1;
      return { play: () => Promise.resolve(), stop: () => undefined };
    },
    schedule: (delayMs, run) => {
      assert.equal(delayMs, 12000);
      restore = run;
      return () => {
        restore = null;
      };
    },
  });

  controller.speak('一段话', { source: 'm1', onEnd: () => (ended += 1) });
  await flush();
  assert.equal(controller.state().status, 'unavailable');
  assert.equal(controller.state().message, '语音运行时未就绪');
  // 不回退到系统朗读：没有创建任何播放器。
  assert.equal(players, 0);
  // 调用方拿到的收口回调照常，界面不会卡在准备态。
  assert.equal(ended, 1);

  restore();
  assert.equal(controller.state().status, 'idle');
});

test('播放器创建失败落在失败态，不把异常抛给调用方', async () => {
  const controller = createSpeechController({
    requestAudio: async () => ({ audioBase64: 'AA==', mimeType: 'audio/wav' }),
    createPlayer: () => {
      throw new Error('音频设备不可用');
    },
    schedule: () => () => undefined,
  });

  controller.speak('一段话', { source: 'm1' });
  await flush();
  assert.equal(controller.state().status, 'error');
  assert.equal(controller.state().message, '音频设备不可用');
});

test('播放失败落在失败态并带上原因', async () => {
  const controller = createSpeechController({
    requestAudio: async () => ({ audioBase64: 'AA==', mimeType: 'audio/wav' }),
    createPlayer: (payload, events) => ({
      play: async () => {
        events.onFailed(new Error('音频播放失败'));
      },
      stop: () => undefined,
    }),
    schedule: () => () => undefined,
  });

  controller.speak('一段话', { source: 'm1' });
  await flush();
  assert.equal(controller.state().status, 'error');
  assert.equal(controller.state().message, '音频播放失败');
});

test('没有可念的文字时不发请求，超长正文在客户端截断', async () => {
  const requests = [];
  const controller = createSpeechController({
    requestAudio: (text) => {
      requests.push(text);
      return new Promise(() => undefined);
    },
    createPlayer: () => ({ play: () => Promise.resolve(), stop: () => undefined }),
    schedule: () => () => undefined,
  });

  controller.speak('```\n只有代码\n```', { source: 'empty' });
  assert.equal(requests.length, 0);

  // 超长正文在客户端截断，并按句分段请求：第一段立即发，下一段预取。
  controller.speak('文'.repeat(MAX_SPEECH_CHARS + 500), { source: 'long' });
  assert.ok(requests.length >= 1, '应至少请求第一段');
  const total = requests.reduce((sum, text) => sum + text.length, 0);
  assert.ok(total <= MAX_SPEECH_CHARS, `总长应被截到 ${MAX_SPEECH_CHARS}，实际 ${total}`);
  for (const text of requests) {
    assert.ok(text.length <= SEGMENT_MAX_CHARS, '每段都应短于分段上限');
  }
});

test('长文按句分段：每段都不超过上限，且拼回原文顺序不变', () => {
  const long = '第一句话说明背景。第二句话给出结论！第三句话补充限制？' + '再来一段很长的描述，'.repeat(30) + '收尾。';
  const segments = splitSpeechSegments(long, 60);
  assert.ok(segments.length > 1, '长文应被切成多段');
  for (const segment of segments) {
    assert.ok(segment.length <= 60, `分段超长: ${segment.length}`);
  }
  assert.equal(segments.join(''), long);
});

test('短句合并成一段，避免一句一个合成请求', () => {
  const segments = splitSpeechSegments('好。行。可以。', 120);
  assert.deepEqual(segments, ['好。行。可以。']);
  assert.deepEqual(splitSpeechSegments('   '), []);
});

test('分段朗读逐段合成：播完一段再请求下一段，不并发抢跑', async () => {
  const requests = [];
  const played = [];
  const controller = createSpeechController({
    requestAudio: (text) => {
      requests.push(text);
      return Promise.resolve({ audioBase64: 'AA==', mimeType: 'audio/wav' });
    },
    createPlayer: (_payload, events) => ({
      play: () => {
        played.push(1);
        // 播完立刻收口，让控制器推进下一段。
        queueMicrotask(() => events.onEnded());
        return Promise.resolve();
      },
      stop: () => undefined,
    }),
    schedule: () => () => undefined,
  });

  const long = ['甲'.repeat(70) + '。', '乙'.repeat(70) + '。'].join('');
  controller.speak(long);
  for (let index = 0; index < 12; index += 1) await Promise.resolve();
  assert.equal(played.length, 2, '两段都应被播放');
  assert.equal(controller.state().status, 'idle');
});
