import test from 'node:test';
import assert from 'node:assert/strict';
import {
    attachmentMeta,
    attachmentMetaText,
    formatAttachmentSize,
} from '../src/conversation/attachmentMeta.ts';

test('类别词按扩展名给出，图片以图片标记为准', () => {
    assert.equal(attachmentMeta({ name: 'AI Voice凭证.xlsx', image: false, bytes: 5_120 }).label, 'Excel');
    assert.equal(attachmentMeta({ name: '规格.DOCX', image: false, bytes: null }).label, 'Word');
    assert.equal(attachmentMeta({ name: 'pasted-标题-20261007-1048.md', image: false, bytes: null }).label, 'Markdown');
    assert.equal(attachmentMeta({ name: 'shot.png', image: true, bytes: null }).label, '图片');
});

test('判不出类别时不硬塞无信息量的词', () => {
    assert.equal(attachmentMeta({ name: 'archive.bin', image: false, bytes: 10 }).label, '');
    assert.equal(attachmentMeta({ name: 'no-extension', image: false, bytes: 10 }).label, '');
});

test('体量按 1024 进制给出，缺字节时不给数字', () => {
    assert.equal(formatAttachmentSize(512), '512B');
    assert.equal(formatAttachmentSize(5_120), '5KB');
    assert.equal(formatAttachmentSize(1_572_864), '1.5MB');
    assert.equal(formatAttachmentSize(null), '');
});

test('元信息成行：两类都缺时整行不渲染', () => {
    assert.equal(
        attachmentMetaText(attachmentMeta({ name: 'AI Voice凭证.xlsx', image: false, bytes: 5_120 })),
        'Excel · 5KB',
    );
    assert.equal(
        attachmentMetaText(attachmentMeta({ name: 'pasted-说明.md', image: false, bytes: null })),
        'Markdown',
        '没有体量就只显示类别，不补占位',
    );
    assert.equal(attachmentMetaText(attachmentMeta({ name: 'archive.bin', image: false, bytes: null })), '');
});
