import test from 'node:test';
import assert from 'node:assert/strict';
import { retryPromptOf } from '../src/conversation/transcript/retryPrompt.ts';

/** 只带重试需要的三个字段：角色、正文与附件。 */
const user = (text, attachments = []) => ({ role: 'user', text, attachments });
const assistant = (text) => ({ role: 'assistant', text, attachments: [] });
const failure = (text) => ({ role: 'failure', text, attachments: [] });

test('重试取这一轮发起时的那条用户消息', () => {
  const prompt = retryPromptOf([
    user('把构建脚本拆开'),
    assistant('先看一眼现有脚本。'),
    failure('本轮执行未完成：upstream header timeout'),
  ]);
  assert.ok(prompt !== null);
  assert.equal(prompt.text, '把构建脚本拆开');
  assert.deepEqual(prompt.attachments, []);
});

test('运行中补发的消息不抢占重试请求', () => {
  const attachments = [{ path: 'a.png', name: 'a.png' }];
  const prompt = retryPromptOf([
    user('跑一遍验证', attachments),
    assistant('开始执行。'),
    user('顺便看下日志'),
    failure('本轮执行未完成'),
  ]);
  assert.ok(prompt !== null);
  assert.equal(prompt.text, '跑一遍验证');
  assert.equal(prompt.attachments, attachments);
});

test('没有用户单元的轮次给不出重试请求', () => {
  assert.equal(retryPromptOf([assistant('续跑'), failure('失败')]), null);
  assert.equal(retryPromptOf([]), null);
});

test('空正文且没有附件时不提供重试', () => {
  assert.equal(retryPromptOf([user('   '), failure('失败')]), null);
});

test('只有附件的消息仍然可以重试', () => {
  const attachments = [{ path: 'shot.png', name: 'shot.png' }];
  const prompt = retryPromptOf([user('', attachments), failure('失败')]);
  assert.ok(prompt !== null);
  assert.equal(prompt.text, '');
  assert.equal(prompt.attachments, attachments);
});
