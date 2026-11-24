import test from 'node:test';
import assert from 'node:assert/strict';
import { reflectionNoticeText } from '../src/conversation/reflectionNotice.ts';

test('提示文案原样透传，不改写「本轮」', () => {
    assert.equal(
        reflectionNoticeText('记忆反思：本轮线索已入候选池，攒够阈值或池龄到点后统一整理'),
        '记忆反思：本轮线索已入候选池，攒够阈值或池龄到点后统一整理',
    );
    assert.equal(
        reflectionNoticeText('记忆反思：本轮新增、淘汰或归档 2 条记忆'),
        '记忆反思：本轮新增、淘汰或归档 2 条记忆',
    );
    assert.equal(
        reflectionNoticeText('记忆反思：本轮新增、淘汰或归档 2 条记忆，另有 1 条候选未写入'),
        '记忆反思：本轮新增、淘汰或归档 2 条记忆，另有 1 条候选未写入',
    );
});

test('别的提示与别的字一个不动', () => {
    assert.equal(
        reflectionNoticeText('记忆反思：本轮线索已入候选池，本轮不跑反思'),
        '记忆反思：本轮线索已入候选池，本轮不跑反思',
    );
    assert.equal(
        reflectionNoticeText('记忆反思：候选池已清空'),
        '记忆反思：候选池已清空',
    );
    assert.equal(reflectionNoticeText('上下文已压缩：本轮保留了 12 条消息'), '上下文已压缩：本轮保留了 12 条消息');
});

test('空串、纯空白与没有提示都返回 null，不占位', () => {
    assert.equal(reflectionNoticeText(null), null);
    assert.equal(reflectionNoticeText(undefined), null);
    assert.equal(reflectionNoticeText(''), null);
    assert.equal(reflectionNoticeText('   \n\t '), null);
    // 前后留白收掉，提示本身仍要能看见。
    assert.equal(
        reflectionNoticeText('  记忆反思：本轮线索已入候选池  '),
        '记忆反思：本轮线索已入候选池',
    );
});
