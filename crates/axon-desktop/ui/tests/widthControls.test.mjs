import test from 'node:test';
import assert from 'node:assert/strict';
import {
  calculateDraggedWidth,
  CONTENT_MIN,
  DEFAULT_WIDTH,
  readWidthPreference,
  resolveContentWidth,
  WIDTH_PREF_KEY,
} from '../src/components/conversation/widthControls.ts';

test('读取本地持久化宽度偏好：有效数字正常返回，非法或缺失回退 null', () => {
  const store = new Map();
  const mockStorage = {
    getItem: (key) => store.get(key) ?? null,
    setItem: (key, val) => store.set(key, String(val)),
    removeItem: (key) => store.delete(key),
  };

  assert.equal(readWidthPreference(mockStorage), null);

  mockStorage.setItem(WIDTH_PREF_KEY, '960');
  assert.equal(readWidthPreference(mockStorage), 960);

  mockStorage.setItem(WIDTH_PREF_KEY, '-100');
  assert.equal(readWidthPreference(mockStorage), null);

  mockStorage.setItem(WIDTH_PREF_KEY, 'abc');
  assert.equal(readWidthPreference(mockStorage), null);
});

test('求解对话列宽度：有偏好时在安全上下限内夹紧，无偏好时使用默认基准', () => {
  const containerWidth = 1400;

  // 无偏好时使用默认 840
  assert.equal(resolveContentWidth(containerWidth, null), DEFAULT_WIDTH);

  // 有偏好且在合理区间
  assert.equal(resolveContentWidth(containerWidth, 1000), 1000);

  // 偏好过小低于 CONTENT_MIN (640) 时夹到 640
  assert.equal(resolveContentWidth(containerWidth, 500), CONTENT_MIN);

  // 容器极窄（如 700px）时，max 会被夹到 CONTENT_MIN
  assert.equal(resolveContentWidth(700, 1200), CONTENT_MIN);
});

test('拖拽宽度计算：左右手柄向外拖拽增大，向内拖拽减小且两边对称', () => {
  const base = 840;

  // 右侧手柄向右拖动 50px (deltaX = 50)，总宽度增加 100px
  assert.equal(calculateDraggedWidth(base, 50, 'right'), 940);

  // 右侧手柄向左拖动 50px (deltaX = -50)，总宽度减少 100px
  assert.equal(calculateDraggedWidth(base, -50, 'right'), 740);

  // 左侧手柄向左拖动 50px (deltaX = -50)，总宽度增加 100px
  assert.equal(calculateDraggedWidth(base, -50, 'left'), 940);

  // 左侧手柄向右拖动 50px (deltaX = 50)，总宽度减少 100px
  assert.equal(calculateDraggedWidth(base, 50, 'left'), 740);
});
