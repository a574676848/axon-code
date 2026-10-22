import test from 'node:test';
import assert from 'node:assert/strict';
import {
  activeReferenceToken,
  directoryArgument,
  matchReferenceCandidates,
  referenceInsertion,
  structuredInputBlock,
  visibleUserText,
} from '../src/conversation/references.ts';

const entry = (path, kind = 'file') => ({ name: path.split('/').pop(), path, kind });

test('只有紧贴光标、前面是空白的 @ 才算活跃引用', () => {
    const text = '看看 @src/ma';
    assert.deepEqual(activeReferenceToken(text, text.length), {
        start: 3,
        end: text.length,
        query: 'src/ma',
        directory: 'src/',
        prefix: 'ma',
    });

    // 邮箱地址里的 @ 不是引用。
    const mail = '发到 a@b.com';
    assert.equal(activeReferenceToken(mail, mail.length), null);
    // 已经敲了空格：这一次引用结束了。
    const space = '引用 @a b';
    assert.equal(activeReferenceToken(space, space.length), null);
    // 只有一个 @：在根目录里挑。
    assert.deepEqual(activeReferenceToken('@', 1), {
        start: 0,
        end: 1,
        query: '',
        directory: '',
        prefix: '',
    });
    assert.equal(activeReferenceToken('没有引用', 4), null);
});

test('候选排序与 TUI 的路径补全同序：目录加权、精确 > 前缀 > 子串', () => {
    const entries = [
        entry('a-model-b'), // 子串命中
        entry('modelX'), // 前缀命中
        entry('model'), // 精确命中
        entry('docs/guide.md'),
    ];
    assert.deepEqual(
        matchReferenceCandidates(entries, 'model').map(({ name }) => name),
        ['model', 'modelX', 'a-model-b'],
    );

    // 同名一轮里目录额外加权：名字更靠后的目录仍然排在文件之前。
    const bonus = [entry('abc'), entry('abd', 'dir'), entry('docs/guide.md')];
    assert.deepEqual(
        matchReferenceCandidates(bonus, 'ab').map(({ name }) => name),
        ['abd', 'abc'],
    );

    assert.equal(matchReferenceCandidates(entries, '不存在').length, 0);
    assert.ok(matchReferenceCandidates(entries, '').length <= 8, '候选条数要有上限');
});

test('选中目录继续下钻，选中文件结束这一次引用', () => {
    assert.equal(referenceInsertion(entry('src', 'dir')), 'src/');
    assert.equal(referenceInsertion(entry('src/model.rs')), 'model.rs ');
    assert.equal(directoryArgument('src/'), 'src');
    assert.equal(directoryArgument(''), '');
});

test('提交时只带上正文里仍然看得见的引用', () => {
    const text = '改一下 @src/model.rs 和 @README.md';
    const block = structuredInputBlock(text, ['src/model.rs', 'README.md', 'src/other.rs']);
    assert.equal(block, '\n\n[结构化输入]\n已引用文件：src/model.rs\n已引用文件：README.md');

    // 用户把引用文字删掉了：不再提交它。
    assert.equal(structuredInputBlock('改一下', ['src/model.rs']), '');
    // 重复引用只提交一次。
    assert.equal(
        structuredInputBlock('@README.md', ['README.md', 'README.md']),
        '\n\n[结构化输入]\n已引用文件：README.md',
    );
});

test('回放时只展示用户敲的那一段，不展示结构化输入块', () => {
    const content = '改一下 @src/model.rs\n\n[结构化输入]\n已引用文件：src/model.rs';
    assert.equal(visibleUserText(content), '改一下 @src/model.rs');
    // 正文里恰好写了同名标题、但后面不是引用行：原样保留。
    const lookalike = '标题\n\n[结构化输入]\n这是我的说明';
    assert.equal(visibleUserText(lookalike), lookalike);
    assert.equal(visibleUserText('普通消息'), '普通消息');
});

test('只有附件、没有正文时，附件路径成为这条消息的正文', () => {
    const paths = ['C:\\tmp\\shot.png', 'C:\\tmp\\notes.txt'];
    assert.equal(
        structuredInputBlock('', [], paths),
        '[结构化输入]\n已附加文件：C:\\tmp\\shot.png\n已附加文件：C:\\tmp\\notes.txt',
    );
    // 有正文时不再追加：消息本身已经说明了意图。
    assert.equal(structuredInputBlock('看下这张图', [], paths), '');
    // 没有附件也没有引用：这条消息确实没有可提交的内容。
    assert.equal(structuredInputBlock('', [], []), '');
});

test('回放时同样剥掉附件块，气泡里只剩附件卡', () => {
    assert.equal(visibleUserText('[结构化输入]\n已附加文件：/tmp/a.png'), '');
    // 提交链路会裁掉前导空白，附带说明的消息块也可能跟在正文之后。
    assert.equal(
        visibleUserText('看下这张图\n\n[结构化输入]\n已附加文件：/tmp/a.png'),
        '看下这张图',
    );
    // 同名标题后面不是块行时不能误剥。
    const lookalike = '[结构化输入]\n这是我的说明';
    assert.equal(visibleUserText(lookalike), lookalike);
});
