/**
 * 问答卡片的作答草稿。
 *
 * 草稿不放在卡片组件里：卡片会被收起（用户先打字、稍后再答），收起即卸载，
 * 把已选项挂在组件内部会随卸载一起丢掉。因此草稿由输入坞按提问标识持有，
 * 卡片只是这份草稿的受控视图，这里则是它唯一的可变规则所在。
 */

import type { AnswerItem, QuestionItem } from './questions';

/** 一道题的作答草稿。 */
export interface AnswerDraft {
  selected: string[];
  custom: string;
}

/** 一张卡片的作答草稿：按题目标识索引。 */
export type AnswerDraftMap = Record<string, AnswerDraft>;

/** 空草稿；未作答的题一律读它。 */
export const EMPTY_ANSWER_DRAFT: AnswerDraft = { selected: [], custom: '' };

/** 取一道题的草稿；没有时给空草稿。 */
export function draftOf(drafts: AnswerDraftMap, questionId: string): AnswerDraft {
  return drafts[questionId] ?? EMPTY_ANSWER_DRAFT;
}

/**
 * 选中或取消一个选项。
 *
 * 多选是勾选切换；单选是替换，并且清掉自定义回答：两者是同一道题的两种答法，
 * 同时留着会让提交结果取决于后端如何取舍。反向不成立，先选一项再补自定义说明
 * 是常见搭配，那时选项要留着。
 */
export function toggleOption(draft: AnswerDraft, label: string, multiSelect: boolean): AnswerDraft {
  if (multiSelect) {
    const selected = draft.selected.includes(label)
      ? draft.selected.filter((item) => item !== label)
      : [...draft.selected, label];
    return { ...draft, selected };
  }
  return { selected: [label], custom: '' };
}

/** 把草稿汇编成提交给后端的作答数组；空白自定义回答落成 null。 */
export function draftAnswers(
  questions: readonly QuestionItem[],
  drafts: AnswerDraftMap,
): AnswerItem[] {
  return questions.map((question) => {
    const draft = draftOf(drafts, question.id);
    const custom = draft.custom.trim();
    return {
      id: question.id,
      selected: draft.selected,
      custom: custom.length > 0 ? custom : null,
    };
  });
}
