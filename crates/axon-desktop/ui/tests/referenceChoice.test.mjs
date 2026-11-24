import test from 'node:test';
import assert from 'node:assert/strict';

import { choiceAddress, claim, parseChoiceAddress } from '../src/sidebarRight/registry.ts';

test('候选页地址原样往返：中文、空格、竖线、反斜杠都不串位', () => {
    const reference = 'docs/我的 文档|a.md';
    const candidates = ['E:/repo/docs/我的 文档|a.md', 'D:/备份\\\\文档|a.md'];
    const parsed = parseChoiceAddress(choiceAddress(reference, candidates));
    assert.deepEqual(parsed, { reference, candidates });
});

test('候选页地址被认领为 choice 类型，文件地址仍归预览', () => {
    assert.equal(claim(choiceAddress('a.md', ['E:/a/a.md', 'E:/b/a.md']))?.kind, 'choice');
    assert.equal(claim(`axon-resource://file/${encodeURIComponent('a.md')}`)?.kind, 'preview');
});

test('格式不合法的一律解析成 null，不抢别人的地址', () => {
    assert.equal(parseChoiceAddress('axon-resource://file/a.md'), null);
    assert.equal(parseChoiceAddress('axon-resource://choice/只有一段'), null);
    assert.equal(parseChoiceAddress('axon-resource://choice/%E0%A4%A|E:/a/a.md'), null);
});
