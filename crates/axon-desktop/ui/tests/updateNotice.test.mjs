import test from 'node:test';
import assert from 'node:assert/strict';
import { downloadHref, updateHint, updateStatusText } from '../src/updateNotice.ts';

/** 一份「发现新版本」的检测回执，个别字段按用例覆盖。 */
function updateCheck(overrides = {}) {
  return {
    current: '0.1.0',
    latest: 'v0.2.1',
    update_available: true,
    release_url: 'https://example.invalid/releases/v0.2.1',
    download_url: 'https://example.invalid/releases/axon-0.2.1.msi',
    published_at: '2026-10-31T08:00:00Z',
    error: '',
    hint: '',
    ...overrides,
  };
}

test('关于页的结论只有三种口径，外加检测中与未检测两个过渡态', () => {
  assert.equal(updateStatusText(null, true), '正在检测…');
  assert.equal(updateStatusText(null, false), '尚未检测');
  assert.equal(updateStatusText(updateCheck(), false), '发现新版本 v0.2.1');
  assert.equal(updateStatusText(updateCheck({ update_available: false, latest: 'v0.1.0' }), false), '已是最新版本');
  // 检测结论里的失败不区分原因：首屏只给产品口径的一句话。
  assert.equal(
    updateStatusText(updateCheck({ update_available: false, error: 'GitHub 接口访问频率已达上限' }), false),
    '暂时无法检测新版本',
  );
});

test('确认有新版本时侧栏给出入口，提示里写明版本号', () => {
  const hint = updateHint(updateCheck(), null);
  assert.equal(hint.visible, true);
  assert.equal(hint.label, '新版本');
  assert.equal(hint.detail, '新版本：v0.2.1');
  assert.equal(hint.tone, 'link');
  assert.equal(hint.href, 'https://example.invalid/releases/axon-0.2.1.msi');
});

test('没有资产时退回发布页，两个地址都空时整块不出现', () => {
  const releaseOnly = updateHint(updateCheck({ download_url: '' }), null);
  assert.equal(releaseOnly.href, 'https://example.invalid/releases/v0.2.1');

  // 没有落点的入口等于一个点了没反应的按钮，因此宁可不出现。
  const nowhere = updateHint(updateCheck({ download_url: '', release_url: '' }), null);
  assert.equal(nowhere.visible, false);
  assert.equal(nowhere.href, '');
});

test('无更新、检测结论失败与尚未检测时侧栏一律不出现', () => {
  assert.equal(updateHint(updateCheck({ update_available: false }), null).visible, false);
  // 失败原因归关于页：侧栏这一行讲不清一句归因。
  assert.equal(updateHint(updateCheck({ error: '无法连接 GitHub' }), null).visible, false);
  assert.equal(updateHint(null, null).visible, false);
});

test('打开失败时入口转成危险色重试态，原因挂在提示里', () => {
  const hint = updateHint(updateCheck(), '没有可用的浏览器');
  assert.equal(hint.visible, true);
  assert.equal(hint.label, '下载失败');
  assert.equal(hint.tone, 'danger');
  assert.ok(hint.detail.includes('没有可用的浏览器'));
  assert.ok(hint.detail.includes('点击重试'));
  // 重试仍然指向同一个地址，不会因为一次失败换一个落点。
  assert.equal(hint.href, 'https://example.invalid/releases/axon-0.2.1.msi');
});

test('下载落点与是否有更新同源：关于页与侧栏不会指向不同地址', () => {
  assert.equal(downloadHref(updateCheck()), updateHint(updateCheck(), null).href);
  assert.equal(downloadHref(null), '');
  assert.equal(downloadHref(updateCheck({ update_available: false })), '');
  assert.equal(downloadHref(updateCheck({ error: '无法解析 api.github.com' })), '');
});
