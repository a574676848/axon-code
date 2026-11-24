import test from 'node:test';
import assert from 'node:assert/strict';
import {
    LARGE_TEXT_CHAR_THRESHOLD,
    TITLE_MAX_CHARS,
    base64FromText,
    exportMarkdownName,
    isLargeText,
    markdownTitle,
} from '../src/conversation/textExport.ts';

/*
 * 大段文本落盘的换算契约：阈值边界、导出命名、以及大文本的 base64 编码。
 */

test('阈值只在大段文本上生效，边界取严格大于', () => {
    assert.equal(isLargeText('x'.repeat(LARGE_TEXT_CHAR_THRESHOLD)), false);
    assert.equal(isLargeText('x'.repeat(LARGE_TEXT_CHAR_THRESHOLD + 1)), true);
    assert.equal(LARGE_TEXT_CHAR_THRESHOLD, 3000);
});

test('标题优先取标题行，其次取正文首行', () => {
    assert.equal(markdownTitle('# 右键菜单改造\n\n正文'), '右键菜单改造');
    assert.equal(markdownTitle('## 二级标题'), '二级标题');
    assert.equal(markdownTitle('\n\n第一行就是正文\n第二行'), '第一行就是正文');
    assert.equal(markdownTitle('- 列表首项'), '列表首项');
    assert.equal(markdownTitle('> 引用内容'), '引用内容');
});

test('标题行内标记被摘掉，只留可见文字', () => {
    assert.equal(markdownTitle('## **加粗**与`代码`'), '加粗与代码');
    assert.equal(markdownTitle('[链接文字](https://example.com)'), '链接文字');
    assert.equal(markdownTitle('![截图](a.png)'), '截图');
});

test('代码围栏与分割线不算标题', () => {
    assert.equal(markdownTitle('```ts\nconst a = 1;\n```\n\n真正的说明'), '真正的说明');
    assert.equal(markdownTitle('---\n\n正文标题'), '正文标题');
});

test('文件名去掉三平台的非法字符与结尾噪声', () => {
    // 路径分隔符与 Windows 保留字符一律收敛成连字符。
    assert.equal(markdownTitle('a/b\\c:d?e"f|g'), 'a-b-c-d-e-f-g');
    // 标记不是内容：强调符号与 HTML 标签摘掉，不参与命名。
    assert.equal(markdownTitle('**加粗**与<h1>标签</h1>'), '加粗与标签');
    // 结尾的点、空格与标点不进文件名：点与空格在 Windows 上非法。
    assert.equal(markdownTitle('结尾带点...'), '结尾带点');
    assert.equal(markdownTitle('结尾带空格   '), '结尾带空格');
    assert.equal(markdownTitle('结尾是句号。'), '结尾是句号');
});

test('超长标题被截断到上限', () => {
    const title = markdownTitle('一'.repeat(60));
    assert.equal(title.length, TITLE_MAX_CHARS);
    assert.equal(TITLE_MAX_CHARS, 24);
});

test('取不到可读文字时返回 null', () => {
    assert.equal(markdownTitle(''), null);
    assert.equal(markdownTitle('   \n\n  '), null);
    assert.equal(markdownTitle('```\ncode only\n```'), null);
    assert.equal(markdownTitle('***'), null);
});

test('导出名带标题与时间戳，取不到标题时退回用途加时间戳', () => {
    const now = new Date(2026, 9, 7, 10, 48, 14);
    assert.equal(exportMarkdownName('assistant', '# 会话回顾\n正文', now), '会话回顾-20261007-1048.md');
    // 粘贴件保留来源前缀：它是临时产物，用户需要知道这东西是怎么来的。
    assert.equal(exportMarkdownName('pasted', '一段粘贴的日志', now), 'pasted-一段粘贴的日志-20261007-1048.md');
    assert.equal(exportMarkdownName('assistant', '   ', now), 'assistant-20261007-1048.md');
    assert.equal(exportMarkdownName('pasted', '', now), 'pasted-20261007-1048.md');
});

test('导出名里不出现任何平台的非法字符', () => {
    const name = exportMarkdownName('assistant', '# 路径 C:\\tmp\\a.txt 与 a/b:c', new Date(2026, 0, 2, 3, 4, 5));
    assert.doesNotMatch(name, /[<>:"/\\|?*]/);
    assert.match(name, /\.md$/);
});

test('base64 编码对多字节文本往返一致', () => {
    const text = '中文与 emoji 🙃 混排，外加 ASCII';
    const decoded = Buffer.from(base64FromText(text), 'base64').toString('utf8');
    assert.equal(decoded, text);
});

test('超大文本仍能编码，不会因为展开实参过多而爆栈', () => {
    // 20 万字符：一次性 String.fromCharCode(...bytes) 会直接爆栈，分块编码必须扛住。
    const text = 'a'.repeat(200_000);
    const encoded = base64FromText(text);
    assert.equal(encoded.length, Math.ceil(200_000 / 3) * 4);
    assert.equal(Buffer.from(encoded, 'base64').toString('utf8'), text);
});
