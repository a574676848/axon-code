import test from 'node:test';
import assert from 'node:assert/strict';
import { splitSkillTokens } from '../src/conversation/skillTokens.ts';

/** 把切分结果压成便于断言的形态。 */
const shape = (text, names) =>
    splitSkillTokens(text, names).map((segment) => (segment.skill ? `[${segment.text}]` : segment.text)).join('');

const names = ['browser-skill', 'anydoc', 'plan'];

test('精确命中技能名的 /name 才点亮，空白与前后的正文原样保留', () => {
    assert.equal(shape('/anydoc 帮我转成 Markdown', names), '[/anydoc] 帮我转成 Markdown');
    assert.equal(shape('先看一眼 /plan 再动手', names), '先看一眼 [/plan] 再动手');
    assert.equal(shape('第一行\n/browser-skill', names), '第一行\n[/browser-skill]');
    // 正文末尾没有空格同样成立。
    assert.equal(shape('/plan', names), '[/plan]');
    // 一条消息里出现多个：各自点亮，中间的空格不受影响。
    assert.equal(shape('/plan /anydoc', names), '[/plan] [/anydoc]');
});

test('没在名单里的名字不点亮', () => {
    assert.equal(shape('/unknown 干活', names), '/unknown 干活');
    assert.equal(shape('发到 /tmp/x 去', names), '发到 /tmp/x 去');
});

test('路径、后缀与随手写的斜杠都不算指令', () => {
    // 名字后面还黏着路径：这是路径不是指令。
    assert.equal(shape('/plan.md', names), '/plan.md');
    assert.equal(shape('/plan/x', names), '/plan/x');
    // 斜杠前面是词字符：随手写的斜杠。
    assert.equal(shape('看/看', names), '看/看');
    assert.equal(shape('a@b.com/c', names), 'a@b.com/c');
});

test('清单为空时整段原样返回，不做任何切分', () => {
    const segments = splitSkillTokens('/anydoc 干活', []);
    assert.deepEqual(segments, [{ text: '/anydoc 干活', skill: false }]);
    assert.deepEqual(splitSkillTokens('', names), []);
});

test('切分结果拼回去与原文逐字一致', () => {
    const text = '开头 /anydoc 中间 /plan 结尾 /browser-skill';
    assert.equal(splitSkillTokens(text, names).map((segment) => segment.text).join(''), text);
});
