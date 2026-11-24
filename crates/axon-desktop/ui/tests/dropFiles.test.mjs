import test from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_ATTACHMENT_BYTES,
  attachmentSizeError,
  classifyDroppedItems,
  partitionBySize,
} from '../src/conversation/dropFiles.ts';

/*
 * 拖入项的判别契约：目录要被单独挑出来，否则用户只会看到一句读不出来的失败原因。
 */

/** 造一个拖入项；目录同样以 File 形态出现，只有 entry 能区分两者。 */
function item(name, { directory = false } = {}) {
    const file = { name };
    return {
        kind: 'file',
        getAsFile: () => file,
        webkitGetAsEntry: () => ({ isDirectory: directory }),
    };
}

test('文件与目录分开返回', () => {
    const result = classifyDroppedItems({
        items: [item('a.txt'), item('素材', { directory: true }), item('b.png')],
    });
    assert.deepEqual(result.files.map((file) => file.name), ['a.txt', 'b.png']);
    assert.deepEqual(result.directories, ['素材']);
});

test('只拖目录时文件为空但目录被报出', () => {
    const result = classifyDroppedItems({ items: [item('src', { directory: true })] });
    assert.deepEqual(result.files, []);
    assert.deepEqual(result.directories, ['src']);
});

test('拿不到目录信息时按文件处理', () => {
    const plain = { kind: 'file', getAsFile: () => ({ name: 'c.md' }) };
    const result = classifyDroppedItems({ items: [plain] });
    assert.deepEqual(result.files.map((file) => file.name), ['c.md']);
    assert.deepEqual(result.directories, []);
});

test('没有 items 时退回 files', () => {
    const result = classifyDroppedItems({ files: [{ name: 'd.log' }, { name: 'e.json' }] });
    assert.deepEqual(result.files.map((file) => file.name), ['d.log', 'e.json']);
    assert.deepEqual(result.directories, []);
});

test('忽略非文件类型的拖入项（例如拖来的文本片段）', () => {
    const result = classifyDroppedItems({
        items: [{ kind: 'string', getAsFile: () => null }],
        files: [{ name: 'f.txt' }],
    });
    assert.deepEqual(result.files.map((file) => file.name), ['f.txt']);
});

test('超过 8 MiB 的文件在读之前就被挡下', () => {
    const files = [
        { name: '小.png', size: 1024 },
        { name: '大.mov', size: MAX_ATTACHMENT_BYTES + 1 },
        { name: '刚好.bin', size: MAX_ATTACHMENT_BYTES },
    ];
    const { accepted, rejected } = partitionBySize(files);
    assert.deepEqual(accepted.map((file) => file.name), ['小.png', '刚好.bin']);
    assert.deepEqual(rejected.map((file) => file.name), ['大.mov']);
});

test('超限的说法写明上限与文件名', () => {
    assert.equal(attachmentSizeError('a.png', 1024), null);
    assert.equal(attachmentSizeError('大.mov', MAX_ATTACHMENT_BYTES + 1), '超过 8 MB 不支持：大.mov');
    // 名字拿不到时也要给一句能读的结论。
    assert.equal(attachmentSizeError('', MAX_ATTACHMENT_BYTES + 1), '超过 8 MB 不支持：未命名文件');
});
