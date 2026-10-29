import test from 'node:test';
import assert from 'node:assert/strict';
import {
  EMPTY_ANSWER_DRAFT,
  draftAnswers,
  draftOf,
  toggleOption,
} from '../src/conversation/questionDraft.ts';

const questions = [
  { id: 'q1', question: '生产环境发布？' },
  { id: 'q2', question: '要带哪些验证？' },
];

test('多选是勾选切换，单选点其他项是替换并清掉自定义回答', () => {
  const empty = EMPTY_ANSWER_DRAFT;
  assert.deepEqual(empty, { selected: [], custom: '' });

  // 多选：同一个选项点两次回到未选。
  const once = toggleOption(empty, 'A', true);
  const twice = toggleOption(toggleOption(once, 'B', true), 'A', true);
  assert.deepEqual(once.selected, ['A']);
  assert.deepEqual(twice.selected, ['B']);

  // 单选：换一项即替换，且不与自定义回答并存。
  const withCustom = { selected: ['A'], custom: '补充说明' };
  assert.deepEqual(toggleOption(withCustom, 'B', false), { selected: ['B'], custom: '' });
});

test('单选再点已选中的那一项即取消选择，自定义回答按原文保留', () => {
  // 单选也有反选：点第二下回到未答，不靠「换一项」绕。
  assert.deepEqual(toggleOption({ selected: ['同意'], custom: '' }, '同意', false), {
    selected: [],
    custom: '',
  });

  // 取消只撤掉这一项：用户另外填写的说明不是选项，不被顺手清掉。
  assert.deepEqual(
    toggleOption({ selected: ['同意'], custom: '灰度先放 5%' }, '同意', false),
    { selected: [], custom: '灰度先放 5%' },
  );

  // 换成另一项仍是替换，并清掉自定义回答（同一道题只留一种答法）。
  assert.deepEqual(
    toggleOption({ selected: ['同意'], custom: '灰度先放 5%' }, '拒绝', false),
    { selected: ['拒绝'], custom: '' },
  );
});

test('作答按题目定义汇编，空白自定义回答落成 null 而不是空串', () => {
  const drafts = {
    q1: { selected: ['同意'], custom: '  灰度先放 5%  ' },
    q2: { selected: [], custom: '   ' },
  };
  assert.deepEqual(draftAnswers(questions, drafts), [
    { id: 'q1', selected: ['同意'], custom: '灰度先放 5%' },
    { id: 'q2', selected: [], custom: null },
  ]);
});

test('未作答的题目取稳定空草稿，跳过后其他题的已选项不受影响', () => {
  assert.equal(draftOf({}, 'q1'), EMPTY_ANSWER_DRAFT);

  // 跳过 q1 只清空它自己：q2 的已选项与自定义回答原样保留。
  const drafts = {
    q1: { selected: ['A'], custom: 'x' },
    q2: { selected: ['B'], custom: '保留' },
  };
  const skipped = { ...drafts, q1: EMPTY_ANSWER_DRAFT };
  assert.deepEqual(draftAnswers(questions, skipped), [
    { id: 'q1', selected: [], custom: null },
    { id: 'q2', selected: ['B'], custom: '保留' },
  ]);
});
