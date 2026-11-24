import test from 'node:test';
import assert from 'node:assert/strict';
import {
    decodeImageDestination,
    imagePathCandidates,
    imagePathScanCandidates,
    isImagePath,
    localImagePath,
    parseImageNode,
    resolveWorkspacePath,
    stripDecoration,
} from '../src/components/conversation/localImage.ts';

test('图片判定只看后缀，且后缀必须在可预览清单内', () => {
    for (const value of ['a.png', 'a.JPG', 'a.jpeg', 'a.webp', 'a.gif', 'a.svg', 'a.avif']) {
        assert.equal(isImagePath(value), true, value);
    }
    for (const value of ['a.txt', 'a.md', '.png', 'png', 'a.png.bak', '']) {
        assert.equal(isImagePath(value), false, value);
    }
});

test('剥掉包裹符与尾部句读，路径内部的空格与括号一律不动', () => {
    assert.equal(stripDecoration('`E:\\shots\\a.png`'), 'E:\\shots\\a.png');
    assert.equal(stripDecoration('“E:/shots/a.png”。'), 'E:/shots/a.png');
    assert.equal(stripDecoration('E:/我的 文档/a（1）.png'), 'E:/我的 文档/a（1）.png');
});

test('候选按最贴原样在前，全角冒号只作补充候选', () => {
    assert.deepEqual(imagePathCandidates('E:\\shots\\a.png'), ['E:\\shots\\a.png']);
    assert.deepEqual(imagePathCandidates('E：/shots/a.png'), [
        'E：/shots/a.png',
        'E:/shots/a.png',
    ]);
});

test('裸文本里以图片后缀结尾的那一段会被认出来，含空格的路径可再与后续词拼接', () => {
    const single = imagePathScanCandidates('见 E:\\shots\\a.png 这张图');
    assert.equal(single[0]?.path, 'E:\\shots\\a.png');

    // 路径被空格切开时没有语法边界，只能把各段都试一遍：真正读得出来的那条在候选里。
    const spaced = imagePathScanCandidates('见 E:\\我的 文档\\a.png 这张图').map((item) => item.path);
    assert.ok(spaced.includes('E:\\我的 文档\\a.png'), spaced.join(' | '));
    assert.ok(spaced.includes('文档\\a.png'), spaced.join(' | '));
});

test('没有图片后缀的普通句子不产生候选', () => {
    assert.deepEqual(imagePathScanCandidates('今天改完了三个模块，晚点发版。'), []);
    assert.deepEqual(imagePathScanCandidates('读到 42 条记录'), []);
});

test('Markdown 目标解码一次，并去掉查询与片段后缀', () => {
    assert.equal(decodeImageDestination('docs/a%20b.png'), 'docs/a b.png');
    assert.equal(decodeImageDestination('docs/a.png?raw=1'), 'docs/a.png');
    assert.equal(decodeImageDestination('docs/a.png#L3'), 'docs/a.png');
    // 转义不合法时用原文，不吞掉用户写的路径。
    assert.equal(decodeImageDestination('docs/100%.png'), 'docs/100%.png');
});

test('相对路径落到工作区，绝对路径与家目录简写原样上交', () => {
    assert.equal(resolveWorkspacePath('E:/ws', 'assets/a.png'), 'E:/ws/assets/a.png');
    assert.equal(resolveWorkspacePath('E:/ws/', './assets/a.png'), 'E:/ws/assets/a.png');
    assert.equal(resolveWorkspacePath('E:/ws', 'assets\\a.png'), 'E:/ws/assets/a.png');
    assert.equal(resolveWorkspacePath('E:/ws', 'E:/other/a.png'), 'E:/other/a.png');
    assert.equal(resolveWorkspacePath('E:/ws', '~/a.png'), '~/a.png');
    assert.equal(resolveWorkspacePath('E:/ws', '\\\\server\\share\\a.png'), '\\\\server\\share\\a.png');
    // 没有工作区时保持原样：让后端去报读不出来，比前端拼一个假路径诚实。
    assert.equal(resolveWorkspacePath(null, 'assets/a.png'), 'assets/a.png');
});

test('只有本地路径进本地读取通道，远程图片不走', () => {
    assert.equal(localImagePath('E:/ws', 'assets/a.png'), 'E:/ws/assets/a.png');
    assert.equal(localImagePath('E:/ws', 'https://example.com/a.png'), null);
    assert.equal(localImagePath('E:/ws', 'data:image/png;base64,AAAA'), null);
    assert.equal(localImagePath('E:/ws', ''), null);
});

test('图片节点三种写法都解析得出来，非图片节点不认', () => {
    assert.deepEqual(parseImageNode('![架构图](docs/a.png)'), {
        destination: 'docs/a.png',
        alt: '架构图',
    });
    assert.deepEqual(parseImageNode('![](docs/a.png)'), { destination: 'docs/a.png', alt: '' });
    // 路径含空格时的标准转义写法，以及带标题的写法。
    assert.deepEqual(parseImageNode('![图](<E:/我的 文档/a.png>)'), {
        destination: 'E:/我的 文档/a.png',
        alt: '图',
    });
    assert.deepEqual(parseImageNode('![图](<E:/我的 文档/a.png> "标题")'), {
        destination: 'E:/我的 文档/a.png',
        alt: '图',
    });
    assert.equal(parseImageNode('[链接](docs/a.png)'), null);
    assert.equal(parseImageNode('![只有说明]'), null);
    assert.equal(parseImageNode('![](   )'), null);
});
