import test from 'node:test';
import assert from 'node:assert/strict';
import {
  BoundedFrameQueue,
  encodePcm16,
  MAX_QUEUED_FRAMES,
  resampleLinear,
  VOICE_FRAME_SAMPLES,
} from '../src/speech/pcm.ts';
import {
  joinDraft,
  phaseFromServerState,
  VOICE_PHASE_TEXT,
} from '../src/speech/voiceProtocol.ts';

/*
 * 语音采集与呈现的纯逻辑：合帧、重采样、有界队列、六态映射与定稿并稿。
 * 这里不碰麦克风与 WebSocket，因此无头环境也能覆盖这些口径。
 */

test('16 kHz 合帧：100 ms 恰好 1600 个采样点', () => {
  assert.equal(VOICE_FRAME_SAMPLES, 1600);
  assert.equal(MAX_QUEUED_FRAMES, 8);
});

test('重采样把 48 kHz 缩到 16 kHz，长度按比例收敛', () => {
  const input = new Float32Array(480);
  for (let index = 0; index < input.length; index += 1) input[index] = index / 480;
  const output = resampleLinear(input, 48_000, 16_000);
  assert.equal(output.length, 160);
  assert.ok(output[0] >= 0 && output[0] < 0.01);
  // 相同采样率不做无谓拷贝。
  assert.equal(resampleLinear(input, 16_000, 16_000), input);
});

test('s16le 编码在正负半轴分别夹紧到 32767 与 -32768', () => {
  const encoded = encodePcm16(new Float32Array([-2, -1, 0, 1, 2]));
  assert.deepEqual(Array.from(encoded), [-32768, -32768, 0, 32767, 32767]);
});

test('上行队列有界 8 帧，满时丢最旧并累计计数', () => {
  const queue = new BoundedFrameQueue(8);
  for (let index = 0; index < 10; index += 1) {
    queue.push(new Int16Array([index]).buffer);
  }
  assert.equal(queue.size, 8);
  assert.equal(queue.droppedFrames, 2);
  assert.equal(new Int16Array(queue.shift())[0], 2, '丢掉的应该是最早的两帧');
  queue.clear();
  assert.equal(queue.size, 0);
});

test('九个协议相位归到界面六态，未知相位按待机', () => {
  assert.equal(phaseFromServerState('arming'), 'arming');
  assert.equal(phaseFromServerState('listening'), 'listening');
  assert.equal(phaseFromServerState('endpointing'), 'thinking');
  assert.equal(phaseFromServerState('thinking'), 'thinking');
  assert.equal(phaseFromServerState('speaking'), 'speaking');
  assert.equal(phaseFromServerState('interrupted'), 'interrupted');
  assert.equal(phaseFromServerState('idle'), 'idle');
  assert.equal(phaseFromServerState('error'), 'idle');
  assert.equal(phaseFromServerState('closed'), 'idle');
  assert.equal(phaseFromServerState(undefined), 'idle');
  assert.deepEqual(Object.values(VOICE_PHASE_TEXT), [
    '准备中',
    '聆听',
    '思考',
    '说话',
    '被打断',
  ]);
});

test('定稿只追加不覆盖，中文之间不补空格，其它情况补一个空格', () => {
  assert.equal(joinDraft('', '你好'), '你好');
  assert.equal(joinDraft('先说一句。', '你好'), '先说一句。你好');
  assert.equal(joinDraft('hello', 'world'), 'hello world');
  assert.equal(joinDraft('hello ', 'world'), 'hello world');
  assert.equal(joinDraft('看这个', 'fix bug'), '看这个 fix bug');
  assert.equal(joinDraft('hello', '  '), 'hello');
});
