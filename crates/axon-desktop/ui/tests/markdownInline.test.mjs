import test from 'node:test';
import assert from 'node:assert/strict';
import { scanInline } from '../src/components/conversation/markdownInlineScan.ts';

/*
 * 行内标记扫描的回归测试。
 *
 * 这条测试守的是一个真实事故：行内扫描曾共用一个带 `g` 标志的正则，而 `lastIndex`
 * 记在正则对象自身；强调类记号要递归扫描它的内部文本，递归会把 `lastIndex` 改到内层
 * 字符串的位置，外层回来继续扫描就从一个更早的位置重新命中同一个记号，光标不前进、
 * 记号无限增长，最终把渲染进程撑到内存不足（真机表现为整个界面卡死、只能落到引导页）。
 *
 * 因此这里断言的核心不是样式，而是「一定会终止，且记号数有界」。
 */

test('嵌套强调必须终止且记号有界（曾因共享 lastIndex 无限循环）', () => {
    const cases = [
        '**粗体**',
        '**外层 *内层* 收尾**',
        '~~删除 **加粗** 收尾~~',
        '*斜体* 与 `代码` 与 **粗体**',
        '**a *b* c** 尾随 *d*',
        '**a** **b** **c** **d**',
        '*[链接](https://example.com) 里的斜体*',
    ];
    for (const source of cases) {
        const tokens = scanInline(source);
        assert.ok(tokens.length > 0, `应扫出记号：${source}`);
        assert.ok(
            tokens.length <= source.length,
            `记号数必须有界（源 ${source.length} 字符，实得 ${tokens.length} 个）`,
        );
        // 记号必须落在源文本内，且不越过它的长度。
        for (const token of tokens) {
            assert.ok(token.index >= 0 && token.index + token.text.length <= source.length, `记号越界：${source}`);
        }
    }
});

test('记号按出现顺序返回，起点单调不减', () => {
    const tokens = scanInline('**一** 与 *二* 与 `三`');
    assert.deepEqual(
        tokens.map((token) => token.kind),
        ['strong', 'em', 'code'],
    );
    for (let index = 1; index < tokens.length; index += 1) {
        assert.ok(tokens[index].index > tokens[index - 1].index, '起点必须递增');
    }
});

test('强调类记号带出内部文本，代码里的标记不被当作标记', () => {
    const [strong] = scanInline('**粗**');
    assert.equal(strong.kind, 'strong');
    assert.equal(strong.inner, '粗');

    const code = scanInline('`**不是粗体**`');
    assert.equal(code.length, 1, '整段应只扫出一个记号');
    assert.equal(code[0].kind, 'code');
    assert.equal(code[0].inner, '**不是粗体**');
});
