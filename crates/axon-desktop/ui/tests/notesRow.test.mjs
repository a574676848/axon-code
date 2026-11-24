import test from 'node:test';
import assert from 'node:assert/strict';
import { notesRowContent, notesWriteOutput } from '../src/components/conversation/toolPresentation.ts';

/** 工具侧的两种便签写入回执都以这句起头，正文跟在空行之后。 */
const writeReceipt = '便签已整文件写入：checkpoint.md（3793 字节）。';
const appendReceipt = '便签已追加：checkpoint.md（新增 24 字节，当前 3817 字节）。';

test('写入便签回执拆成回执与正文两段', () => {
    const body = '# Goal Checkpoint\n\n## 目标\n修复 Jira 工单';
    assert.deepEqual(notesWriteOutput(`${writeReceipt}\n\n${body}`), { receipt: writeReceipt, body });
});

test('追加回执同样拆出这一次追加的正文', () => {
    const body = '补充下一步动作';
    assert.deepEqual(notesWriteOutput(`${appendReceipt}\n\n${body}`), {
        receipt: appendReceipt,
        body,
    });
});

test('正文里自带空行时只在回执之后切第一刀', () => {
    const body = '# 标题\n\n第一段\n\n第二段';
    assert.equal(notesWriteOutput(`${writeReceipt}\n\n${body}`)?.body, body);
});

test('超过内联上限的回执不带正文分隔，仍按回执原文展示', () => {
    const oversized = `${writeReceipt}正文超过内联上限 4096 字节，用 builtin.notes_read_file 按行回读。`;
    assert.equal(notesWriteOutput(oversized), null);
});

test('只有回执没有正文时不拆', () => {
    assert.equal(notesWriteOutput(writeReceipt), null);
    assert.equal(notesWriteOutput(`${writeReceipt}\n\n   \n`), null);
});

test('不是便签写入回执的正文不接管', () => {
    assert.equal(notesWriteOutput('命令输出\n\n第二段'), null);
    assert.equal(notesWriteOutput('便签 checkpoint.md 当前为空。\n\n首行'), null);
});

test('CRLF 回执与正文同样能拆', () => {
    const body = '第一行\n第二行';
    assert.deepEqual(notesWriteOutput(`${writeReceipt}\r\n\r\n${body}`), { receipt: writeReceipt, body });
});

// ---------------------------------------------------------------- 便签行展开内容

test('读取便签：输出本身就是正文，没有独立回执行', () => {
    const note = '# Goal Checkpoint\n\n- 下一步：跑门禁';
    assert.deepEqual(notesRowContent('读取便签', note), { body: note, receipt: null });
});

test('读取空便签：那一句回执也按正文渲染，不铺代码块', () => {
    const empty = '便签 checkpoint.md 当前为空。';
    assert.deepEqual(notesRowContent('读取便签', empty), { body: empty, receipt: null });
});

test('写入与追加：正文与回执分开，正文在前块渲染', () => {
    const body = '## 目标\n修复工单';
    assert.deepEqual(notesRowContent('写入便签', `${appendReceipt}\n\n${body}`), {
        receipt: appendReceipt,
        body,
    });
});

test('写入正文超限：只有回执说明，绝不把说明当正文', () => {
    const oversized = `${writeReceipt}正文超过内联上限 4096 字节，用 builtin.notes_read_file 按行回读。`;
    assert.deepEqual(notesRowContent('写入便签', oversized), { body: null, receipt: oversized });
});

test('写入失败：错误文案当回执展示', () => {
    const failure = '便签路径不合法：../escape.md';
    assert.deepEqual(notesRowContent('写入便签', failure), { body: null, receipt: failure });
});

test('列举与检索便签交给原来的输出块，不按正文渲染', () => {
    assert.deepEqual(notesRowContent('匹配便签', 'checkpoint.md\t3793 字节'), {
        body: null,
        receipt: null,
    });
    assert.deepEqual(notesRowContent('检索便签', 'checkpoint.md:3: ## 目标'), {
        body: null,
        receipt: null,
    });
});

test('非便签行与空输出都不产出内容', () => {
    assert.deepEqual(notesRowContent('执行', 'cargo test'), { body: null, receipt: null });
    assert.deepEqual(notesRowContent('写入便签', '   '), { body: null, receipt: null });
});
