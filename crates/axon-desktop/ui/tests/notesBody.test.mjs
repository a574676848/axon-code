import test from 'node:test';
import assert from 'node:assert/strict';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import { NotesBody } from '../src/components/conversation/NotesBody.tsx';

const render = (text) => renderToStaticMarkup(React.createElement(NotesBody, { text }));

test('便签正文按 Markdown 渲染，原始标记不外泄', () => {
    const html = render('# Goal Checkpoint\n\n- 一项进展\n- 二项进展');

    // 渲染器把一级标题落成正文段落加标题档位类（不是原生 h1），这里锁的是档位类。
    assert.match(html, /<p[^>]*text-heading-1[^>]*>Goal Checkpoint<\/p>/, '标题必须走标题档位：' + html);
    assert.equal((html.match(/<li/g) ?? []).length, 2, '两条列表项：' + html);
    assert.ok(!html.includes('# Goal'), '井号标记必须被渲染掉：' + html);
});

test('限高与滚动条：容器自带 320px 上限与可见滚动条', () => {
    const html = render('正文');

    assert.ok(html.includes('max-h-80'), '限高与代码块同一档：' + html);
    assert.ok(html.includes('overflow-y-auto'), '超出限高时内部滚动：' + html);
    assert.ok(html.includes('scroll-visible'), '长内容必须让人看见还有：' + html);
});

test('任务列表渲染成勾选框，不显示方括号标记', () => {
    const html = render('- [x] 门禁已跑\n- [ ] 真机待验');

    assert.ok(html.includes('aria-hidden="true"'), '勾选框是装饰图形：' + html);
    assert.ok(!html.includes('[x]'), '方括号标记必须被吃掉：' + html);
    assert.ok(!html.includes('[ ]'), '方括号标记必须被吃掉：' + html);
});

test('行内代码保留为代码片段，不断行成普通段落', () => {
    const html = render('用 `cargo test` 收口');

    assert.match(html, /<code[^>]*>cargo test<\/code>/, html);
});
