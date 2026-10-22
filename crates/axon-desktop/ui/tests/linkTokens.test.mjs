import test from 'node:test';
import assert from 'node:assert/strict';

import {
    compactUrlLabel,
    expandLinks,
    findUrls,
    linkHost,
    liveLinks,
    tokenizePastedText,
} from '../src/conversation/linkTokens.ts';
import {
    splitComposerSegments,
    splitMessageSegments,
} from '../src/conversation/textSegments.ts';

/** 把命中结果压成地址列表。 */
const urls = (text) => findUrls(text).map((match) => match.url);

test('识别地址并给出与原文对齐的区间', () => {
    const text = '看 https://example.com/a?b=1 和 http://127.0.0.1:3143/ok';
    const matches = findUrls(text);
    assert.deepEqual(
        matches.map((match) => match.url),
        ['https://example.com/a?b=1', 'http://127.0.0.1:3143/ok'],
    );
    for (const match of matches) assert.equal(text.slice(match.start, match.end), match.url);
});

test('中文与句读处截断，结尾的不成对括号剥掉', () => {
    assert.deepEqual(urls('看https://example.com/a，然后再说'), ['https://example.com/a']);
    assert.deepEqual(urls('见（https://x.com/a）。'), ['https://x.com/a']);
    assert.deepEqual(urls('结尾是 https://x.com/a.'), ['https://x.com/a']);
    assert.deepEqual(urls('感叹一下 https://x.com/a! 很好'), ['https://x.com/a']);
    assert.deepEqual(urls('(https://x.com/a)'), ['https://x.com/a']);
    // 问号是查询串的开头，不剥：剥掉等于改了粘进来的 URL。
    assert.deepEqual(urls('https://x.com/a?'), ['https://x.com/a?']);
    // 成对的括号属于地址本身。
    assert.deepEqual(
        urls('https://en.wikipedia.org/wiki/Foo_(bar)'),
        ['https://en.wikipedia.org/wiki/Foo_(bar)'],
    );
});

test('只认 http 与 https，裸域名与其他协议不接管', () => {
    assert.deepEqual(urls('ftp://x.com/a www.example.com mailto:a@b.com'), []);
    assert.deepEqual(urls('只有 https:// 没有正文'), []);
});

test('站点名去掉 www，保留端口；短地址去掉查询串与结尾斜杠', () => {
    assert.equal(linkHost('https://www.example.com:8443/a?q=1'), 'example.com:8443');
    assert.equal(compactUrlLabel('https://www.example.com/a/b/?q=1#frag'), 'example.com/a/b');
    assert.equal(compactUrlLabel('https://example.com/'), 'example.com');
    assert.equal(linkHost('https://[::1]:3000/a'), '::1:3000');
});

test('粘贴地址换成一个短占位符，展开回来与原文逐字一致', () => {
    const pasted =
        '看这个 https://www.toutiao.com/article/7693683419241354787/?app=news_article 很有意思';
    const step = tokenizePastedText(pasted, '', []);
    assert.equal(step.text, '看这个 [toutiao.com] 很有意思');
    assert.deepEqual(step.tokens, [
        {
            placeholder: '[toutiao.com]',
            url: 'https://www.toutiao.com/article/7693683419241354787/?app=news_article',
        },
    ]);
    assert.equal(expandLinks(step.text, step.tokens), pasted);
});

test('同一地址复用占位，同站的不同地址加序号，草稿里已有的同样文字也让它让位', () => {
    const first = tokenizePastedText('https://a.com/x', '', []);
    const second = tokenizePastedText('https://a.com/y', '', first.tokens);
    assert.equal(second.text, '[a.com #2]');
    // 同一地址再粘一次：复用原来的占位，不新增清单项。
    const again = tokenizePastedText('https://a.com/x', '', second.tokens);
    assert.equal(again.text, '[a.com]');
    assert.equal(again.tokens.length, 2);
    // 草稿里恰好写着同样的文字：换一个带序号的后缀，避免展开时误伤正文。
    const conflicted = tokenizePastedText('https://a.com/x', '我写过 [a.com]', []);
    assert.equal(conflicted.text, '[a.com #2]');
});

test('没有地址的粘贴不接管', () => {
    assert.equal(tokenizePastedText('普通文本', '', []), null);
});

test('仍然出现在正文里的占位按位置排序，删掉的不再出现', () => {
    const tokens = [
        { placeholder: '[a.com]', url: 'https://a.com/' },
        { placeholder: '[b.com]', url: 'https://b.com/' },
    ];
    assert.deepEqual(
        liveLinks('先 [b.com] 后 [a.com]', tokens).map((token) => token.placeholder),
        ['[b.com]', '[a.com]'],
    );
    assert.deepEqual(liveLinks('只剩 [a.com]', tokens).map((token) => token.placeholder), [
        '[a.com]',
    ]);
    assert.deepEqual(liveLinks('一个都没有', tokens), []);
});

test('同一个占位出现两次时两处都展开', () => {
    const tokens = [{ placeholder: '[a.com]', url: 'https://a.com/x' }];
    assert.equal(
        expandLinks('[a.com] 与 [a.com]', tokens),
        'https://a.com/x 与 https://a.com/x',
    );
});

test('消息气泡先认地址再认技能，地址里的斜杠不拆成技能', () => {
    const segments = splitMessageSegments('看 https://example.com/plan/a ，再 /plan', ['plan']);
    assert.deepEqual(
        segments.map((segment) => segment.kind),
        ['text', 'link', 'text', 'skill'],
    );
    assert.equal(segments[1].url, 'https://example.com/plan/a');
});

test('输入框底衬只认占位，裸地址保持普通文字', () => {
    const tokens = [{ placeholder: '[a.com]', url: 'https://a.com/x' }];
    const segments = splitComposerSegments('看 [a.com] 与 https://b.com/y', [], tokens);
    assert.deepEqual(
        segments.map((segment) => segment.kind),
        ['text', 'link', 'text'],
    );
    assert.equal(segments[1].url, 'https://a.com/x');
    assert.equal(segments[2].text, ' 与 https://b.com/y');
});
