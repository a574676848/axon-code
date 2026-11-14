import test from 'node:test';
import assert from 'node:assert/strict';

import {
    buildReferenceIndex,
    isAbsoluteReference,
    normalizePath,
} from '../src/resources/referenceIndex.ts';
import { resolveReference } from '../src/resources/resolveReference.ts';
import { sessionReferenceIndex } from '../src/components/conversation/referenceAnchors.ts';

/** 用一批路径建索引，返回解析函数。 */
const resolverFor = (...paths) => {
    const index = buildReferenceIndex(paths);
    return (reference) => resolveReference(reference, index);
};

test('规范化路径：设备前缀、反斜杠与重复分隔符收敛，UNC 的双斜杠保住', () => {
    assert.equal(normalizePath('//server/share/a'), '//server/share/a');
    assert.equal(normalizePath('\\\\?\\E:\\zbg\\repo'), 'E:/zbg/repo');
    assert.equal(normalizePath('\\\\.\\C:\\a\\\\b\\'), 'C:/a/b');
    assert.equal(normalizePath('./docs/a.md'), 'docs/a.md');
    assert.equal(normalizePath('  E:/a/b/  '), 'E:/a/b');
});

test('绝对路径、UNC 与 ~ 都不需要基准', () => {
    for (const reference of ['E:/zbg/a.md', '~/a.md', '//server/share/a.md', '/opt/a.md']) {
        assert.equal(isAbsoluteReference(reference), true, reference);
    }
    for (const reference of ['docs/a.md', 'a.md', '../a.md', '']) {
        assert.equal(isAbsoluteReference(reference), false, reference);
    }
});

test('绝对引用原样交后端，没有被索引左右', () => {
    const resolve = resolverFor('E:/zbg/repo/docs/a.md');
    const resolution = resolve('E:/other/docs/a.md');
    assert.equal(resolution.status, 'direct');
    assert.equal(resolution.path, 'E:/other/docs/a.md');
    assert.deepEqual(resolution.candidates, []);
});

test('工作区外的相对引用按后缀落到唯一一条绝对路径', () => {
    const resolve = resolverFor(
        'E:/zbg/repo/docs/memory-reflection-redesign.md',
        'E:/zbg/repo/crates/lib.rs',
    );
    const resolution = resolve('docs/memory-reflection-redesign.md');
    assert.equal(resolution.status, 'resolved');
    assert.equal(resolution.path, 'E:/zbg/repo/docs/memory-reflection-redesign.md');
});

test('索引里没有的引用退回既有语义，行为与改动前一致', () => {
    const resolve = resolverFor('E:/zbg/repo/crates/lib.rs');
    const resolution = resolve('docs/unknown.md');
    assert.equal(resolution.status, 'workspace');
    assert.equal(resolution.path, 'docs/unknown.md');
});

test('多候选不猜：退回既有语义并把候选带出去', () => {
    const resolve = resolverFor('E:/a/src/lib.rs', 'E:/b/src/lib.rs');
    const resolution = resolve('src/lib.rs');
    assert.equal(resolution.status, 'ambiguous');
    assert.equal(resolution.path, 'src/lib.rs');
    assert.deepEqual(resolution.candidates, ['E:/a/src/lib.rs', 'E:/b/src/lib.rs']);
});

test('大小写与分隔符差异不影响命中', () => {
    const resolve = resolverFor('E:/Zbg/Repo/Docs/A.md');
    assert.equal(resolve('docs\\a.md').status, 'resolved');
    assert.equal(resolve('DOCS/A.MD').status, 'resolved');
    assert.equal(resolve('./docs/a.md').status, 'resolved');
});

test('只认末段同名的那批：名字不同不会被后缀匹配误伤', () => {
    const resolve = resolverFor('E:/a/docs/b.md');
    assert.equal(resolve('docs/a.md').status, 'workspace');
    assert.equal(resolve('a.md').status, 'workspace');
});

test('没有索引时全部退回既有语义', () => {
    const resolution = resolveReference('docs/a.md', null);
    assert.equal(resolution.status, 'workspace');
    assert.equal(resolution.path, 'docs/a.md');
});

test('空引用与纯空白不制造跳转', () => {
    const resolve = resolverFor('E:/a/docs/b.md');
    assert.equal(resolve('   ').status, 'workspace');
    assert.equal(resolve('').status, 'workspace');
});

test('索引只对逐字相同的路径去重，仅大小写不同各留一条', () => {
    const index = buildReferenceIndex([
        'E:/a/x.md',
        'E:/a/x.md',
        'E:/a/A.md',
        'E:/a/y.md',
        'rel/z.md',
    ]);
    assert.equal(index.size, 3);
    const many = Array.from({ length: 5000 }, (_, position) => `E:/a/f${position}.md`);
    const capped = buildReferenceIndex(many);
    assert.equal(capped.size, 4096);
    // 丢的是最早的：最后一条必须在，第一条必须已经不在。
    assert.equal(capped.match('f4999.md').length, 1);
    assert.equal(capped.match('f0.md').length, 0);
});

test('仅大小写不同的两条各自成条，匹配时先按原样收窄', () => {
    const resolve = resolverFor('E:/repo/Docs/A.md', 'E:/repo/docs/a.md');
    assert.equal(resolve('Docs/A.md').path, 'E:/repo/Docs/A.md');
    assert.equal(resolve('docs/a.md').path, 'E:/repo/docs/a.md');
    // 两处都对不上原样：退回宽松匹配，仍然是歧义，不替用户选。
    const loose = resolve('DOCS/A.MD');
    assert.equal(loose.status, 'ambiguous');
    assert.equal(loose.candidates.length, 2);
});

test('同一末段的极端索引不退化：按目录唯一定位，按裸名只给前几条', () => {
    const many = Array.from({ length: 4096 }, (_, position) => `E:/repo/pkg${position}/index.ts`);
    const index = buildReferenceIndex(many);
    assert.equal(index.size, 4096);
    assert.equal(index.match('pkg2048/index.ts').length, 1);
    assert.equal(index.match('index.ts').length, 8);
    const started = Date.now();
    for (let position = 0; position < 200; position += 1) {
        assert.equal(index.match(`pkg${position * 20}/index.ts`).length, 1);
    }
    const elapsed = Date.now() - started;
    assert.ok(elapsed < 200, `同名末段 200 次反查耗时 ${elapsed}ms，超出预算`);
});

test('会话锚点：改动路径与参数路径都进索引，超大参数不解析', () => {
    const huge = JSON.stringify({ path: 'E:/repo/huge.md', content: 'x'.repeat(20000) });
    const turns = [
        {
            cells: [
                {
                    role: 'tool',
                    editPath: '\\\\?\\E:\\repo\\docs\\a.md',
                    arguments: '{"path":"E:/repo/docs/b.md"}',
                },
            ],
        },
        { cells: [{ role: 'fileChange', path: 'E:/repo/docs/c.md' }] },
        { cells: [{ role: 'tool', editPath: null, arguments: huge }] },
        { cells: [{ role: 'user', attachments: [{ path: 'E:/repo/docs/d.md' }] }] },
    ];
    const index = sessionReferenceIndex(turns);
    for (const reference of ['docs/a.md', 'docs/b.md', 'docs/c.md', 'docs/d.md']) {
        assert.equal(index.match(reference).length, 1, reference);
    }
    // 超过扫描上限的参数不解析：那条路径只能来自改动事件，这里本就没有。
    assert.equal(index.match('huge.md').length, 0);
    // 同一份投影复用同一份索引：数组引用不变就不再重建。
    assert.equal(sessionReferenceIndex(turns), index);
});

test('性能守门：4096 条索引上连续反查 1000 次', () => {
    const many = Array.from({ length: 4096 }, (_, index) => `E:/repo/pkg${index}/docs/file${index}.md`);
    const resolve = resolverFor(...many);
    const started = Date.now();
    for (let index = 0; index < 1000; index += 1) {
        const target = `docs/file${index * 4}.md`;
        const resolution = resolve(target);
        assert.equal(resolution.status, 'resolved');
    }
    const elapsed = Date.now() - started;
    assert.ok(elapsed < 200, `1000 次反查耗时 ${elapsed}ms，超出预算`);
});
