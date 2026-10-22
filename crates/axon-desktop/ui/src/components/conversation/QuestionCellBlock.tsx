import React from 'react';
import type { AnswerItem, QuestionItem, QuestionStatus } from '../../conversation/questions';
import { ChecklistIcon, EditIcon } from '../../icons';
import { useConversationActions } from './actions';
import { MessageActionButton, MessageFooter } from './MessageFooter';

/*
 * 问答单元的呈现。
 *
 * 三件事必须在一条问答行里同时成立：
 * - **状态**：还在等回答、已挂起、已回答各是一档，用户一眼要能分辨；
 * - **挂起正文**：模型在等待时给出的交接说明就是它为什么停在这里，不能只留状态标签；
 * - **入口**：卡片可以被收起（用户想先打字），因此这条行必须能把卡片叫回来。
 *
 * 时间线之外没有第二份事实：这里的取值全部来自投影里的问答单元。
 */

/** 问答单元的取值形状。 */
export interface QuestionCellData {
  callId: string;
  status: QuestionStatus;
  questions: QuestionItem[];
  answers?: AnswerItem[];
  handoff?: string | null;
  /** 提问被记录下来的时刻；与消息页脚同款时间标记，缺值时不出时间。 */
  atMs?: number | null;
}

/** 状态标签与配色：琥珀只用于标记与描边，文字一律取墨水档。 */
function statusLabel(status: QuestionStatus): string {
  if (status === 'settled') return '已回答';
  if (status === 'suspended') return '已挂起';
  if (status === 'extended') return '已延长';
  return '等待回答';
}

function statusTone(status: QuestionStatus): string {
  if (status === 'settled') return 'text-text-secondary';
  if (status === 'suspended') return 'text-gold-ink';
  return 'text-text-primary';
}

/** 一项作答的展示文本。 */
function answerText(answer: AnswerItem): string {
  return answer.custom || answer.selected.join('、') || '（跳过）';
}

export const QuestionCellBlock: React.FC<{ cell: QuestionCellData }> = ({ cell }) => {
  const actions = useConversationActions();
  const settled = cell.status === 'settled';
  const title = cell.questions[0]?.header || '用户决策';
  const askedAtMs = cell.atMs ?? null;

  return (
    <div className="group w-full rounded-md hairline border-border-hairline bg-surface p-3">
      <div className="flex items-center gap-2 min-w-0">
        <ChecklistIcon size={14} className="shrink-0 text-text-tertiary" aria-hidden="true" />
        <span className="min-w-0 truncate text-row font-medium text-text-primary">{title}</span>
        <span
          className={`shrink-0 rounded-xs bg-surface-sunken px-1.5 text-meta ${statusTone(cell.status)}`}
        >
          {statusLabel(cell.status)}
        </span>
      </div>

      <div className="mt-2 text-row-loose text-text-secondary">
        {cell.questions.map((question, index) => (
          <div key={question.id} className="break-words">
            <span className="font-medium text-text-primary">
              {cell.questions.length > 1 ? `${index + 1}. ` : ''}
              {question.question}
            </span>
          </div>
        ))}
      </div>

      {/* 挂起正文：模型为什么停在这里，直接看得到，不必去别处找。 */}
      {cell.handoff !== undefined && cell.handoff !== null && cell.handoff.trim().length > 0 && (
        <p className="mt-2 whitespace-pre-wrap break-words text-meta text-text-tertiary">
          {cell.handoff}
        </p>
      )}

      {cell.answers !== undefined && cell.answers.length > 0 && (
        <div className="mt-2 border-t border-border-hairline pt-2 text-meta text-text-primary">
          <span className="text-text-tertiary">作答结果：</span>
          {cell.answers.map((answer) => (
            <span key={answer.id} className="ml-1 font-medium">
              {answerText(answer)}
            </span>
          ))}
        </div>
      )}

      {/*
        页脚与消息页脚同一份实现：时间常显，操作按钮随悬浮出现。
        待答项的图标取墨水档琥珀色，在一排灰色图标里第一眼就能看到；
        已结算的不给回入口，再打开也提交不了。
      */}
      <MessageFooter atMs={askedAtMs}>
        {!settled && actions !== null && (
          <MessageActionButton
            label="重新打开问答卡片"
            title="把问答卡片叫回输入框继续作答"
            onClick={() => actions.openQuestion(cell.callId)}
            tone={cell.status === 'suspended' ? 'gold' : 'default'}
          >
            <EditIcon size={14} />
          </MessageActionButton>
        )}
      </MessageFooter>
    </div>
  );
};
