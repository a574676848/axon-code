/*
 * 草稿随会话：正文、附件与链接占位各存一格，切换时原样接上，提交后整份作废。
 *
 * 这几条断言的是「输入坞里那份没发出去的内容属于会话」这件事本身，与组件无关：
 * 存、取、清三个动作的语义钉住了，输入坞的接入才有意义。
 */

import test from 'node:test';
import assert from 'node:assert/strict';
import {
  clearComposerDrafts,
  composerAttachmentsFor,
  composerLinksFor,
  composerTextFor,
  forgetComposerDraft,
  rememberComposerAttachments,
  rememberComposerLinks,
  rememberComposerText,
} from '../src/conversation/composerDrafts.ts';

const ATTACHMENT = { path: '/tmp/a.png', name: 'a.png', image: true };
const LINK = { placeholder: '[example.com]', url: 'https://example.com/x', index: 1 };

test('正文按会话各存一份，互不串台', () => {
  clearComposerDrafts();
  rememberComposerText('ws:session-a', '发给 A 的话');
  rememberComposerText('ws:session-b', '发给 B 的话');
  assert.equal(composerTextFor('ws:session-a'), '发给 A 的话');
  assert.equal(composerTextFor('ws:session-b'), '发给 B 的话');
  assert.equal(composerTextFor('ws:session-c'), '');
});

test('首屏草稿单独一格：点进会话再切回首屏，写了一半的话还在', () => {
  clearComposerDrafts();
  rememberComposerText(null, '还没想好发什么');
  assert.equal(composerTextFor(null), '还没想好发什么');
  // 空键与 null 共用一个桶，两处不会各记一份。
  assert.equal(composerTextFor(''), '还没想好发什么');
  assert.equal(composerTextFor('ws:session-a'), '');
});

test('切回原会话拿到的还是原文，空正文不占坑', () => {
  clearComposerDrafts();
  rememberComposerText('ws:session-a', '草稿');
  rememberComposerText('ws:session-a', '');
  assert.equal(composerTextFor('ws:session-a'), '');
});

test('附件与链接占位跟着会话走', () => {
  clearComposerDrafts();
  rememberComposerAttachments('ws:session-a', [ATTACHMENT]);
  rememberComposerLinks('ws:session-a', [LINK]);
  assert.deepEqual(composerAttachmentsFor('ws:session-a'), [ATTACHMENT]);
  assert.deepEqual(composerLinksFor('ws:session-a'), [LINK]);
  assert.deepEqual(composerAttachmentsFor('ws:session-b'), []);
  assert.deepEqual(composerLinksFor('ws:session-b'), []);
  // 交出去的是副本：组件之后改自己的数组，不会改到暂存里那一份。
  const restored = composerAttachmentsFor('ws:session-a');
  assert.notEqual(restored, composerAttachmentsFor('ws:session-a'));
});

test('提交之后整条会话的草稿一起清掉', () => {
  clearComposerDrafts();
  rememberComposerText('ws:session-a', '草稿');
  rememberComposerAttachments('ws:session-a', [ATTACHMENT]);
  rememberComposerLinks('ws:session-a', [LINK]);
  forgetComposerDraft('ws:session-a');
  assert.equal(composerTextFor('ws:session-a'), '');
  assert.deepEqual(composerAttachmentsFor('ws:session-a'), []);
  assert.deepEqual(composerLinksFor('ws:session-a'), []);
});

test('清掉一条会话不影响另一条', () => {
  clearComposerDrafts();
  rememberComposerText('ws:a', '甲');
  rememberComposerText('ws:b', '乙');
  forgetComposerDraft('ws:a');
  assert.equal(composerTextFor('ws:a'), '');
  assert.equal(composerTextFor('ws:b'), '乙');
});
