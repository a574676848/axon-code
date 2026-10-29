import React, { useState } from 'react';
import type { AnswerItem, PendingQuestion } from '../../conversation/questions';
import {
  draftAnswers,
  draftOf,
  toggleOption,
  EMPTY_ANSWER_DRAFT,
  type AnswerDraft,
  type AnswerDraftMap,
} from '../../conversation/questionDraft';
import {
  CheckIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  CloseIcon,
  EditIcon,
} from '../../icons';
import { Button, IconButton } from '../ui/controls';
import { QuestionCountdown } from './QuestionCountdown';

/*
 * 问答占座卡：模型把决策交给用户时，这一张卡抢下输入坞的主展示位。
 *
 * 排版分四层，从上到下读作一句话：
 *   一行弱化的话题标签（模型给的 header）与右上角的读秒、收起；
 *   一行正文，问题之后直接跟弱化的细节说明，两者同一段流；
 *   竖直排列的选项行，行首是序号（多选时为勾选框），选项说明跟在标题之后；
 *   底部一行：左侧翻页，右侧「跳过」与「提交」。
 * 选项竖排而不是横排标签流：选项说明往往是一整句话，横排会被挤成一条长条，
 * 竖排每行一个选项，读起来与选择一一对应。
 *
 * 作答草稿不在这里：卡片会被收起，收起即卸载，草稿由输入坞按提问标识持有，
 * 这里只把用户的选择回报上去（见 conversation/questionDraft）。
 *
 * 卡片何时占位由输入坞判定（见 useQuestionCard）：进入会话、新提问到达、时间线
 * 上的回入口都会让卡片自己回来，只有用户当场收起才让出输入坞。挂起的提问同样占位，
 * 它仍然可以回答。
 *
 * 选项按一次选中、再按一次取消，单选与多选同一套手感（见 toggleOption）。
 * 整卡可聚焦（tabIndex 为 -1）：卡片出现时它就是键盘的落点，Esc 因此
 * 在卡内任意位置都成立，而不是只有在输入框里才成立。
 */

export interface ComposerQuestionCardProps {
  question: PendingQuestion;
  busy?: boolean;
  /** 当前卡片的作答草稿：按题目标识索引。 */
  draft: AnswerDraftMap;
  /** 草稿变化。 */
  onDraftChange: (next: AnswerDraftMap) => void;
  onSubmit: (answers: AnswerItem[]) => void;
  onDismiss: () => void;
}

export const ComposerQuestionCard: React.FC<ComposerQuestionCardProps> = ({
  question,
  busy = false,
  draft,
  onDraftChange,
  onSubmit,
  onDismiss,
}) => {
  const [currentIndex, setCurrentIndex] = useState(0);
  const cardRef = React.useRef<HTMLDivElement>(null);

  /*
   * 卡片占用的是输入坞的位置，原先聚焦的输入框已经不在页面上。
   * 焦点不跟着进来就会落回 body：键盘无处可去，Esc 也不再属于这张卡。
   */
  React.useEffect(() => {
    cardRef.current?.focus();
  }, []);

  const totalQuestions = question.questions.length;
  const currentQ = question.questions[currentIndex] || question.questions[0];

  if (!currentQ) {
    return null;
  }

  const currentAns = draftOf(draft, currentQ.id);
  const isLast = currentIndex >= totalQuestions - 1;

  const writeDraft = (questionId: string, next: AnswerDraft) => {
    onDraftChange({ ...draft, [questionId]: next });
  };

  const handleSelectOption = (label: string) => {
    if (busy) return;
    writeDraft(currentQ.id, toggleOption(currentAns, label, currentQ.multiSelect === true));
  };

  const handleCustomChange = (event: React.ChangeEvent<HTMLTextAreaElement>) => {
    writeDraft(currentQ.id, { ...currentAns, custom: event.target.value });
  };

  const handleSkipQuestion = () => {
    if (busy) return;
    const skipped = { ...draft, [currentQ.id]: EMPTY_ANSWER_DRAFT };
    onDraftChange(skipped);
    if (!isLast) {
      setCurrentIndex((index) => index + 1);
      return;
    }
    // 最后一题（含只有一题）跳过即提交：留空代表跳过，编码成空选择。
    // 否则单题提问时点击没有任何可观察结果。
    onSubmit(draftAnswers(question.questions, skipped));
  };

  const handleFormSubmit = (event?: React.FormEvent) => {
    if (event) event.preventDefault();
    if (busy) return;
    onSubmit(draftAnswers(question.questions, draft));
  };

  const handleKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === 'Escape') {
      event.stopPropagation();
      onDismiss();
      return;
    }
    // 焦点落在按钮上时 Enter 属于那个按钮：这里再提交一次会让同一按键做两件事
    // （例如「跳过」会既跳过又提交）。
    if (event.target instanceof HTMLElement && event.target.closest('button') !== null) {
      return;
    }
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey || isLast)) {
      event.preventDefault();
      handleFormSubmit();
    }
  };

  return (
    <div
      ref={cardRef}
      tabIndex={-1}
      className="rounded-md bg-surface hairline shadow-panel p-4 mb-3 relative"
      onKeyDown={handleKeyDown}
      role="region"
      aria-label="模型决策提问"
    >
      {/* 头部：话题标签与读秒、收起 */}
      <div className="flex items-start justify-between gap-3 mb-1.5">
        <span className="min-w-0 truncate text-meta text-text-tertiary">
          {currentQ.header || '问题需要确认'}
        </span>

        <div className="shrink-0 flex items-center gap-2">
          <QuestionCountdown
            deadlineAtMs={question.deadlineAtMs}
            suspended={question.status === 'suspended'}
          />
          <IconButton label="收起卡片（稍后作答）" onClick={onDismiss}>
            <CloseIcon size={14} />
          </IconButton>
        </div>
      </div>

      {/* 问题正文与细节说明：同一段流，说明弱化跟在问题之后 */}
      <p className="m-0 mb-3 text-body font-medium text-text-primary">
        {currentQ.question}
        {currentQ.detail && <span className="text-text-tertiary"> {currentQ.detail}</span>}
      </p>

      {/* 选项列表：一行一个选项，行首是序号（多选为勾选框），说明跟在标题之后 */}
      {currentQ.options && currentQ.options.length > 0 && (
        <div className="flex flex-col gap-1.5 mb-3">
          {currentQ.options.map((option, index) => {
            const isSelected = currentAns.selected.includes(option.label);
            return (
              <button
                key={option.label}
                type="button"
                disabled={busy}
                // 取消选择是同一个动作：再按一次已选中的选项即回到未选，不改自定义回答。
                aria-pressed={isSelected}
                onClick={() => handleSelectOption(option.label)}
                className={`w-full flex items-start gap-2.5 px-3 py-2 rounded-md border text-left text-row transition-colors duration-100 disabled:cursor-not-allowed disabled:opacity-40 ${
                  isSelected
                    ? 'border-border-selected bg-active'
                    : 'border-border bg-surface hover:bg-hover'
                }`}
              >
                {currentQ.multiSelect ? (
                  <span
                    aria-hidden="true"
                    className={`shrink-0 mt-0.5 w-4 h-4 rounded-xs border-[1.5px] flex items-center justify-center ${
                      isSelected ? 'bg-primary border-primary text-white' : 'border-border-input'
                    }`}
                  >
                    {isSelected && <CheckIcon size={12} />}
                  </span>
                ) : (
                  <span
                    aria-hidden="true"
                    className={`shrink-0 w-5 h-5 rounded-xs flex items-center justify-center text-meta leading-none tabular-nums ${
                      isSelected
                        ? 'bg-primary text-white'
                        : 'bg-surface-sunken text-text-tertiary'
                    }`}
                  >
                    {index + 1}
                  </span>
                )}

                <span className="min-w-0 flex-1">
                  <span className="font-medium text-text-primary">{option.label}</span>
                  {option.description && (
                    /*
                     * 说明比标题弱一档，但选中行整行压在 --color-active 上：三级文本色对
                     * 那块底色只有 4.16:1，达不到正文 AA，因此选中时改用二级文本色。
                     */
                    <span className={isSelected ? 'text-text-secondary' : 'text-text-tertiary'}>
                      {' '}
                      {option.description}
                    </span>
                  )}
                </span>
              </button>
            );
          })}
        </div>
      )}

      {/* 自定义文本输入：多行，说明它是「除了上面选项之外」的答法 */}
      <div
        className={`flex items-start gap-2 px-3 py-2 mb-3 rounded-md border bg-surface transition-colors duration-100 ${
          busy ? 'border-border-input opacity-40' : 'border-border-input focus-within:border-border-selected'
        }`}
      >
        <span aria-hidden="true" className="shrink-0 mt-0.5 text-text-tertiary">
          <EditIcon size={14} />
        </span>
        <textarea
          rows={2}
          value={currentAns.custom}
          onChange={handleCustomChange}
          disabled={busy}
          aria-label="输入你的答案"
          placeholder="输入你的答案"
          className="w-full min-w-0 resize-none bg-transparent border-none outline-none text-row text-text-primary placeholder:text-text-placeholder disabled:cursor-not-allowed"
        />
      </div>

      {/* 底部：左侧翻页，右侧次要动作与主操作 */}
      <div className="flex items-center justify-between gap-3">
        {totalQuestions > 1 ? (
          <div className="flex items-center gap-0.5">
            <IconButton
              label="上一题"
              disabled={busy || currentIndex === 0}
              onClick={() => setCurrentIndex((index) => index - 1)}
            >
              <ChevronLeftIcon size={14} />
            </IconButton>
            <span className="px-1 text-meta tabular-nums text-text-tertiary">
              {currentIndex + 1} / {totalQuestions}
            </span>
            <IconButton
              label="下一题"
              disabled={busy || isLast}
              onClick={() => setCurrentIndex((index) => index + 1)}
            >
              <ChevronRightIcon size={14} />
            </IconButton>
          </div>
        ) : (
          <span />
        )}

        <div className="shrink-0 flex items-center gap-2">
          <Button variant="outline" size="sm" disabled={busy} onClick={handleSkipQuestion}>
            跳过
          </Button>

          {isLast ? (
            <Button variant="primary" size="sm" disabled={busy} onClick={() => handleFormSubmit()}>
              {busy ? '正在提交...' : '提交'}
            </Button>
          ) : (
            <Button
              variant="primary"
              size="sm"
              disabled={busy}
              onClick={() => setCurrentIndex((index) => index + 1)}
            >
              下一题
            </Button>
          )}
        </div>
      </div>
    </div>
  );
};
