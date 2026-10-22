import test from 'node:test';
import assert from 'node:assert/strict';
import {
  activeSlashToken,
  rankCandidateList,
} from '../src/conversation/composerCandidates.ts';

const at = (text, offset) => activeSlashToken(text, offset);

test('斜杠在正文开头、空白之后与标点之后都算活跃指令', () => {
    assert.deepEqual(at('/go', 3), { start: 0, end: 3, query: 'go' });
    assert.deepEqual(at('说一句 /co', 7), { start: 4, end: 7, query: 'co' });
    assert.deepEqual(at('第一行\n/go', 7), { start: 4, end: 7, query: 'go' });
    assert.deepEqual(at('see (/go', 8), { start: 5, end: 8, query: 'go' });
    // 只敲了一条斜杠：候选按原序摊开。
    assert.deepEqual(at('/', 1), { start: 0, end: 1, query: '' });
    // 光标停在正文中间：认的是光标之前那一段，与后面写了什么无关。
    assert.deepEqual(at('/go 后面还有字', 3), { start: 0, end: 3, query: 'go' });
});

test('紧跟词字符的斜杠是普通文本，不是指令', () => {
    // 中文与英文的词字符同样成立：路径与随手写的斜杠都不该弹出候选。
    assert.equal(at('看/看', 3), null);
    assert.equal(at('src/util', 8), null);
    // 邮箱地址里的斜杠同理。
    assert.equal(at('a@b.com/c', 9), null);
});

test('URL 里的两种斜杠不当作指令，参数里的斜杠认开头那条', () => {
    assert.equal(at('https:/go', 9), null);
    assert.equal(at('a//go', 5), null);
    // 协议分隔符后面有空白：那是新的一段，斜杠照常成立。
    assert.deepEqual(at('note: /go', 9), { start: 6, end: 9, query: 'go' });
    // 参数里再写一条斜杠时，中间那条紧跟词字符，因此认的是开头那条。
    assert.deepEqual(at('/model/gpt', 10), { start: 0, end: 10, query: 'model/gpt' });
});

test('空白结束这一段：光标之后写了空格就没有活跃指令', () => {
    assert.equal(at('/go ', 4), null);
    assert.equal(at('/model gpt', 10), null);
    // 光标还在指令名里时，后面的空格不影响判定。
    assert.deepEqual(at('/model gpt', 6), { start: 0, end: 6, query: 'model' });
});

test('查询词为空时原序返回，不做任何过滤', () => {
    const items = [{ name: 'zeta' }, { name: 'alpha' }];
    assert.deepEqual(rankCandidateList(items, ''), items);
    assert.deepEqual(rankCandidateList(items, '   '), items);
    // 返回的是副本：调用方排序不会回头改到调用者手里的数组。
    assert.notEqual(rankCandidateList(items, ''), items);
});

test('前缀命中排在最前，其次是紧凑的有序子序列，最后保持原序', () => {
    const items = [
        { name: 'skill-browser' },
        { name: 'browser-skill' },
        { name: 'bws-tool' },
        { name: 'unrelated' },
    ];
    // bws 是 browser-skill 与 skill-browser 的有序子序列，也是 bws-tool 的前缀；
    // 前缀优先，同为子序列时命中窗口更靠前的在前。
    assert.deepEqual(
        rankCandidateList(items, 'bws').map((item) => item.name),
        ['bws-tool', 'browser-skill', 'skill-browser'],
    );
    // 前缀命中里再按窗口紧凑度排：命中越靠前越紧凑。
    assert.deepEqual(
        rankCandidateList(items, 'browser').map((item) => item.name),
        ['browser-skill', 'skill-browser'],
    );
});

test('匹配大小写无关，查询词两端的空白不计入', () => {
    const items = [{ name: 'Browser-Skill' }, { name: 'other' }];
    assert.deepEqual(rankCandidateList(items, 'BSK').map((item) => item.name), ['Browser-Skill']);
    assert.deepEqual(rankCandidateList(items, '  bsk  ').map((item) => item.name), ['Browser-Skill']);
});

test('命中窗口更紧凑的候选排在前面', () => {
    const items = [{ name: 'a-b-c' }, { name: 'ab-c' }];
    assert.deepEqual(
        rankCandidateList(items, 'abc').map((item) => item.name),
        ['ab-c', 'a-b-c'],
    );
});

test('凑不出有序子序列的候选一律不出现', () => {
    const items = [{ name: 'plan' }, { name: 'review' }];
    assert.deepEqual(rankCandidateList(items, 'zz'), []);
    // 顺序反了同样不算命中：子序列要求按序吃到。
    assert.deepEqual(rankCandidateList(items, 'nalp'), []);
});
