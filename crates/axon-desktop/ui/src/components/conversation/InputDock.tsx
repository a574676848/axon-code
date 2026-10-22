import React from 'react';
import { ArrowUpIcon, PlusIcon, StopIcon } from '../../icons';
import { structuredInputBlock } from '../../conversation/references';
import type { AttachmentRef } from '../../conversation/transcript';
import { caretOnFirstLine, caretOnLastLine } from '../../conversation/inputHistory';
import { openWebLink } from '../../conversation/openWebLink';
import { ComposerTray } from './ComposerTray';
import { useConversationActions } from './actions';
import { ComposerNotice, ComposerWorkingLine } from './ComposerStatus';
import { ImageLightbox } from './ImageLightbox';
import { ComposerCandidateLayer } from './ComposerCandidateLayer';
import { composerOptionId } from './ComposerCandidates';
import { ComposerPicker } from './ComposerPicker';
import { ComposerTextArea } from './ComposerTextArea';
import { useExpiringNotice } from './composerNotice';
import { useComposerAttachments } from './useComposerAttachments';
import { useComposerEditing, type CaretRange } from './useComposerEditing';
import { useComposerLinks } from './useComposerLinks';
import { useInputHistory } from './useInputHistory';
import { useReferenceCompletion } from './useReferenceCompletion';
import { useSkillNames } from '../../conversation/skillCatalog';
import { useSlashCompletion } from './useSlashCompletion';
import { ContextMenu, useContextMenu } from '../ui/ContextMenu';
import type { AnswerItem, PendingQuestion } from '../../conversation/questions';
import type { AnswerDraftMap } from '../../conversation/questionDraft';
import { ComposerQuestionCard } from './ComposerQuestionCard';
import { Dialog } from '../ui/overlays';

/*
 * 悬浮输入坞。
 *
 * 只接入有真实后端来源的能力：多行输入与提交、模型与推理等级、附件、`@` 文件引用
 * 与 `/` 指令。每块能力各有归属：编辑与撤销历史在 useComposerEditing，附件落盘的三条
 * 路径（粘贴、拖放、系统选择器）在 useComposerAttachments，模型与推理等级在
 * ComposerPicker，两组候选在 useReferenceCompletion 与 useSlashCompletion。
 * 本组件只负责把它们摆到位并处理键盘。
 */

/** 输入框最多向上伸展的行数。 */
const MAX_ROWS = 6;
/** 单行高度（与 15px/22px 正文一致）。 */
const LINE_HEIGHT = 22;
/** 上下内边距合计。 */
const VERTICAL_PADDING = 20;

/** 窄列下的稳定占位文案：一行放得下，不在 472px 时折成两行再被裁掉。 */
const PLACEHOLDER_READY = '给 Axon 发送消息，@ 引用文件';
/** 占座卡展开时的占位：用户在卡片里作答，输入框让位。 */
const PLACEHOLDER_OCCUPIED = '请在上方向模型回复，或收起卡片自由输入...';
/** 会话未就绪。 */
const PLACEHOLDER_DISABLED = '会话未就绪...';
/** 键盘提示：只在列宽足够时出现。 */
const KEYBOARD_HINT = 'Shift + Enter 换行';

/** 无草稿时的稳定空值：每次渲染换一个新对象只会让受控的卡片白重渲染。 */
const EMPTY_DRAFT: AnswerDraftMap = {};

/** 两组候选菜单的列表 id：输入框用它声明 aria-controls 与 aria-activedescendant。 */
const SLASH_LIST_ID = 'composer-slash-candidates';
const REFERENCE_LIST_ID = 'composer-reference-candidates';

export interface InputDockProps {
  /** 当前工作区根目录；`@` 文件引用按它做作用域索引。 */
  workspaceRoot: string | null;
  /** 是否可提交。 */
  enabled: boolean;
  /** 正在提交。 */
  busy: boolean;
  /**
   * 紧凑变体：子代理输入条共用这一份实现。
   *
   * 只关掉与主会话无关的两项（模型选择与附件），输入、提交、键盘与历史全部沿用，
   * 因此不会再出现第三套字号与发送按钮。
   */
  compact?: boolean;
  /** 键盘提示是否显示：列宽不足时收起，避免那一行先折行再被裁。 */
  hintVisible?: boolean;
  /** 占位文案覆盖（子代理输入条要给「把下一步交给谁」）。 */
  placeholder?: string;
  /** 卡片下方提示文案覆盖。 */
  hint?: string;
  /** 当前主模型；紧凑变体不需要。 */
  model?: string;
  /** 可选模型清单。 */
  models?: string[];
  /** 配置写入期间暂停模型与推理等级切换。 */
  configBusy?: boolean;
  configLocked?: boolean;
  /** 当前推理等级。 */
  reasoningEffort?: string | null;
  /** 切换主模型。 */
  onModelChange?: (value: string) => void;
  /** 切换推理等级。 */
  onReasoningEffortChange?: (value: string) => void;
  /** 提交正文。 */
  onSubmit: (text: string, attachments: AttachmentRef[], structuredInput: string) => void;
  /** 外部错误提示。 */
  error: string | null;
  /** 工作状态行：与 TUI 的 composer dock 同位置同口径；为空时不渲染。 */
  working: { primary: string; elapsedMs: number | null; queued: number } | null;
  /** 中断正在运行的一轮。 */
  onInterrupt: () => void;
  /** 当前待决或挂起的模型提问。 */
  activeQuestion?: PendingQuestion | null;
  /** 提交问答作答。 */
  onAnswerQuestion?: (callId: string, answers: AnswerItem[]) => void;
  /**
   * 重新打开卡片的请求：外部入口（时间线上的「重新打开」）每点一次就递增一次，
   * 卡片据此从收起态回到输入坞的主展示位。
   */
  reopenToken?: number;
  /**
   * 作答提交失败的原因（例如提问已在别处结算）。
   *
   * 这类失败是服务端对一次操作的拒绝，不是输入坞的常态提示，因此用弹窗告知，
   * 不占用输入框上方的错误位。
   */
  questionError?: string | null;
  /** 关闭作答失败弹窗。 */
  onDismissQuestionError?: () => void;
}

export const InputDock: React.FC<InputDockProps> = ({
  workspaceRoot,
  enabled,
  busy,
  compact = false,
  hintVisible = false,
  placeholder,
  hint,
  model = '',
  models = [],
  configBusy = false,
  configLocked = false,
  reasoningEffort = null,
  onModelChange,
  onReasoningEffortChange,
  onSubmit,
  error,
  working,
  onInterrupt,
  activeQuestion = null,
  onAnswerQuestion,
  reopenToken = 0,
  questionError = null,
  onDismissQuestionError,
}) => {
  /** 占座卡是否被用户显式收起。 */
  const [cardDismissed, setCardDismissed] = React.useState(false);
  /** 卡片是否由外部入口叫回主展示位；它同时让挂起的提问也能作答。 */
  const [cardForced, setCardForced] = React.useState(false);
  /** 上一次看到的提问标识，用于在出现新提问时自动重新弹出占座卡。 */
  const lastQuestionCallId = React.useRef<string | null>(null);
  /** 作答草稿：按提问标识持有，卡片收起再打开时已选项仍在。 */
  const [drafts, setDrafts] = React.useState<Record<string, AnswerDraftMap>>({});

  React.useEffect(() => {
    if (activeQuestion && activeQuestion.callId !== lastQuestionCallId.current) {
      lastQuestionCallId.current = activeQuestion.callId;
      setCardDismissed(false);
      setCardForced(false);
    }
  }, [activeQuestion]);

  /** 外部叫回：收起的卡片重新占座，挂起中的提问也因此可以作答。 */
  React.useEffect(() => {
    if (reopenToken === 0) return;
    setCardDismissed(false);
    setCardForced(true);
  }, [reopenToken]);

  /** 没有待答提问时不留草稿：结算过的作答留着既没有消费者，也会随会话时长堆积。 */
  React.useEffect(() => {
    if (activeQuestion !== null) return;
    setCardForced(false);
    setDrafts((current) => (Object.keys(current).length === 0 ? current : {}));
  }, [activeQuestion]);

  /** 光标位置：`@` 与 `/` 两种 token 的判定都依赖它，而 textarea 的值本身不携带光标。 */
  const [caret, setCaret] = React.useState(0);
  /** 程序化写入正文后要落到的光标位置；等 DOM 更新完再应用。 */
  const [pendingCaret, setPendingCaret] = React.useState<number | null>(null);
  /** 输入坞自身的错误（历史写入失败）；附件与拖放的提示在附件控制器里。 */
  const [localError, setLocalError] = React.useState<string | null>(null);
  /*
   * 它是这一次写入没成的回执，不是常驻状态：与同一行上的外部错误同规则，到点自己退场，
   * 否则输入框上方会一直留着一行与当前输入无关的红字。
   */
  useExpiringNotice(localError, () => setLocalError(null));
  /** 正在放大查看的附件图片；为空时不显示浮层。 */
  const [previewing, setPreviewing] = React.useState<string | null>(null);
  /** 会话动作：把附件在右侧栏打开。 */
  const actions = useConversationActions();
  const areaRef = React.useRef<HTMLTextAreaElement>(null);
  /** 候选菜单的锚点：菜单贴在卡片上沿，与它同宽。 */
  const cardRef = React.useRef<HTMLDivElement>(null);
  const {
    attachments,
    notice,
    dragging,
    notify,
    handlePaste,
    pick,
    remove,
    clear,
    stageLargeText,
    dropHandlers,
  } = useComposerAttachments();
  /*
   * 右键菜单的粘贴要先经过编辑控制器，而链接控制器又依赖编辑控制器的写入，
   * 因此用一个稳定的转发把两边接上：编辑控制器先拿到入口，链接控制器随后挂上。
   */
  const pasteProjection = React.useRef<
    (pasted: string, range: CaretRange) => { text: string; caret: number } | null
  >(() => null);
  const preparePaste = React.useCallback(
    (pasted: string, range: CaretRange) =>
      compact ? null : pasteProjection.current(pasted, range),
    [compact],
  );
  const editing = useComposerEditing({
    areaRef,
    onLargeText: stageLargeText,
    onNotice: (text) => notify(text, 'danger'),
    onCaretRequest: setPendingCaret,
    preparePaste,
  });
  const menu = useContextMenu();
  const text = editing.text;
  /** 链接占位：粘贴时收成短占位符，提交时展开回完整地址。 */
  const links = useComposerLinks({
    text,
    commit: editing.commit,
    requestCaret: setPendingCaret,
  });
  pasteProjection.current = links.projectPaste;
  /** `@` 文件引用、`/` 指令与输入历史各一个控制器：取数与缓存都不属于渲染。 */
  const references = useReferenceCompletion({ workspaceRoot, text, caret });
  const slash = useSlashCompletion({ text, caret });
  /** 已安装技能名：输入框用它点亮正文里的 `/name`，与候选菜单读同一份清单。 */
  const skillNames = useSkillNames();
  const recall = useInputHistory();

  const hasText = text.trim().length > 0;
  /** 有内容待发：运行中也照样成立；那时这条消息进入排队，由后端在安全边界插入。 */
  const hasContent = hasText || attachments.length > 0;
  const sendable = hasContent && enabled && !busy;
  /**
   * 运行中且没有待发内容时，主按钮让位给中断。
   *
   * 与 TUI 的 Enter/Esc 分工一致：Enter 排队发送，Esc 中断。提交在途时（busy）
   * 必须保持发送按钮，否则「点一下发送」会在回执回来之前变成「点一下中断」。
   */
  const showStop = working !== null && !hasContent && !busy;

  /**
   * 输入法组合中。
   *
   * 拼音串是输入法的中间态，不是用户写下的正文：它一变长就可能多折一行，
   * 输入坞随之长高一行，正文视口就矮一行，贴底跟随再把整块正文顶上去。
   * 组合期间每敲一个键都这样来一次，读起来就是上方正文一直在跳。
   * 因此组合期间不重新量高度（超出的部分由输入框自己滚动），确认之后再量一次。
   */
  const [composing, setComposing] = React.useState(false);

  /**
   * 正文变高时把输入框一起撑开，最多撑到 MAX_ROWS 行。
   *
   * 先归零再按 scrollHeight 设定：不归零的话高度只增不减，删掉几行之后
   * 输入框会留着一块空白。
   */
  const resize = React.useCallback(() => {
    const area = areaRef.current;
    if (area === null) return;
    area.style.height = 'auto';
    const max = LINE_HEIGHT * MAX_ROWS + VERTICAL_PADDING;
    area.style.height = `${Math.min(area.scrollHeight, max)}px`;
  }, []);

  React.useEffect(() => {
    if (composing) return;
    resize();
  }, [composing, resize, text]);

  /*
   * 程序化写入正文之后再把光标落到位。
   *
   * 直接在新值写下去之前调 setSelectionRange 只会作用在旧文本上（被夹到旧长度），
   * 因此这里等这一次渲染落地后再设置，并把焦点交回输入框。
   */
  React.useEffect(() => {
    if (pendingCaret === null) return;
    const area = areaRef.current;
    if (area !== null) {
      area.focus();
      area.setSelectionRange(pendingCaret, pendingCaret);
      setCaret(pendingCaret);
    }
    setPendingCaret(null);
  }, [pendingCaret]);

  const submit = () => {
    if ((!hasText && attachments.length === 0) || busy || !enabled) return;
    // 引用的正文照旧展示给用户，只有真正发出去的正文带上结构化输入块。
    // 链接占位在这里展开回原始地址：模型与消息记录拿到的都是一段完整 URL。
    const outgoing = links.expand(text);
    // 附件同样进结构化输入块：只有附件、没有正文时，它就是这条消息的正文。
    const paths = attachments.map((attachment) => attachment.path);
    onSubmit(outgoing, attachments, structuredInputBlock(outgoing, references.references, paths));
    // 与 TUI 同一份历史：记下来，之后按上键就能取回（含发送失败的那一次）。
    recall.record(outgoing, setLocalError);
    // 清空草稿并丢弃撤销历史：这一份已经发出去了，撤销它只会让人以为消息没发出去。
    editing.reset();
    clear();
    links.reset();
    references.reset();
    slash.reset();
    recall.reset();
    setCaret(0);
    if (areaRef.current !== null) {
      areaRef.current.style.height = `${LINE_HEIGHT + VERTICAL_PADDING}px`;
    }
  };

  /** 采纳一个 `@` 候选项：目录继续下钻，文件结束这一次引用。 */
  const acceptReference = React.useCallback(
    (index: number) => {
      const step = references.accept(index, text);
      if (step === null) return;
      editing.commit(step.text);
      setPendingCaret(step.caret);
    },
    [editing, references, text],
  );

  /** 采纳一条斜杠指令：把指令名写进正文，光标停在名字之后。 */
  const acceptSlash = React.useCallback(
    (index: number) => {
      const step = slash.accept(index, text);
      if (step === null) return;
      editing.commit(step.text);
      setPendingCaret(step.caret);
    },
    [editing, slash, text],
  );

  /**
   * 当前打开的那一组候选。
   *
   * `@` 与 `/` 两段 token 都要求光标之前没有空白，因此同一时刻至多有一组；
   * 键盘与无障碍声明读的是同一份结论，不会出现「界面收起、键盘还在选」。
   */
  const openCandidates =
    references.options.length > 0
      ? {
          listId: REFERENCE_LIST_ID,
          isReference: true,
          count: references.options.length,
          index: references.index,
        }
      : slash.open
        ? {
            listId: SLASH_LIST_ID,
            isReference: false,
            count: slash.options.length,
            index: slash.index,
          }
        : null;

  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.nativeEvent.isComposing) return;

    /*
     * 撤销与恢复由这里接管：语义见 composerHistory（逐字、粘贴整段、中文按词）。
     * 复制、剪切与粘贴交给浏览器原生行为，它会照常派发一次变更，
     * 因此撤销粒度自动正确，也不必自己实现剪贴板往返。
     */
    if (event.ctrlKey || event.metaKey) {
      const key = event.key.toLowerCase();
      if (key === 'z' && !event.shiftKey) {
        event.preventDefault();
        editing.undo();
        return;
      }
      if (key === 'y' || (key === 'z' && event.shiftKey)) {
        event.preventDefault();
        editing.redo();
        return;
      }
      if (key === 'a') {
        event.preventDefault();
        editing.selectAll();
        return;
      }
    }

    /*
     * 候选项浮层打开时，键盘优先服务于选择，与 TUI 的 popup 一致：
     * 上下移动、Enter 或 Tab 采纳、Esc 与 Shift + Tab 收起（收起只作用于这一条 token，
     * 正文一变候选照常回来，因此它不会变成「这个功能以后都不弹了」）。
     */
    if (openCandidates !== null) {
      const { isReference, count, index: active } = openCandidates;
      const setActive = isReference ? (next: number) => references.setIndex(next) : slash.setIndex;
      const accept = isReference ? acceptReference : acceptSlash;
      const dismissCandidates = isReference ? references.dismiss : slash.dismiss;
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        const delta = event.key === 'ArrowDown' ? 1 : -1;
        setActive((active + delta + count) % count);
        return;
      }
      if (event.key === 'Enter' || (event.key === 'Tab' && !event.shiftKey)) {
        event.preventDefault();
        accept(active);
        return;
      }
      if (event.key === 'Escape' || (event.key === 'Tab' && event.shiftKey)) {
        event.preventDefault();
        dismissCandidates();
        return;
      }
    }
    /*
     * 历史输入：与 TUI 同口径，只有光标在第一行按上、最后一行按下才翻历史，
     * 多行草稿里的方向键仍然用于移动光标。
     */
    if (
      (event.key === 'ArrowUp' || event.key === 'ArrowDown') &&
      !event.shiftKey &&
      !event.altKey &&
      !event.ctrlKey &&
      !event.metaKey
    ) {
      if (event.key === 'ArrowUp' && caretOnFirstLine(text, caret)) {
        // 上键在首行固定用于翻历史（与 TUI 一致）；没历史时只是什么也不发生。
        event.preventDefault();
        void recall.previous(text).then((recalled) => {
          if (recalled === null) return;
          editing.commit(recalled);
          setPendingCaret(recalled.length);
        });
        return;
      }
      if (event.key === 'ArrowDown' && caretOnLastLine(text, caret)) {
        const recalled = recall.next();
        if (recalled !== null) {
          event.preventDefault();
          editing.commit(recalled);
          setPendingCaret(recalled.length);
          return;
        }
      }
    }
    if (event.key !== 'Enter' || event.shiftKey) return;
    event.preventDefault();
    submit();
  };

  const effort = reasoningEffort ?? 'medium';

  // 判定是否占座：处于 pending/extended 且未被用户收起时占座；
  // 由外部入口叫回来的卡片同样占座，挂起中的提问只有这一条路径能回到主展示位。
  const occupiesDock = Boolean(
    activeQuestion !== null &&
      !cardDismissed &&
      (cardForced || activeQuestion.status === 'pending' || activeQuestion.status === 'extended'),
  );

  return (
    /*
     * 正常流式布局，不再绝对定位：输入坞浮在正文之上时，
     * 滚动中的正文会从输入框背面穿过去，边界感只能靠留白假装。
     */
    <div
      {...dropHandlers}
      className={`shrink-0 flex justify-center bg-canvas ${
        compact ? 'px-2 pb-2.5 pt-2' : 'px-4 pb-4 pt-3'
      }`}
    >
      <div className="w-full max-w-chat">
        {/* 占座卡：抢占输入坞主展示位 */}
        {occupiesDock && activeQuestion ? (
          <ComposerQuestionCard
            question={activeQuestion}
            busy={busy}
            draft={drafts[activeQuestion.callId] ?? EMPTY_DRAFT}
            onDraftChange={(next) =>
              setDrafts((current) => ({ ...current, [activeQuestion.callId]: next }))
            }
            onSubmit={(answers) => onAnswerQuestion?.(activeQuestion.callId, answers)}
            onDismiss={() => setCardDismissed(true)}
          />
        ) : (
          <>
            {error !== null && <ComposerNotice tone="danger" text={error} />}
            {localError !== null && <ComposerNotice tone="danger" text={localError} />}
            {/* 附件与拖放的提示：说明刚才发生了什么，不是错误，因此按语气分色。 */}
            {notice !== null && (
              <ComposerNotice tone={notice.tone === 'danger' ? 'danger' : 'secondary'} text={notice.text} />
            )}
            {/* 工作状态行：与 TUI 的 composer dock 同位置同口径，运行期间一直保留。 */}
            {working !== null && (
              <ComposerWorkingLine
                primary={working.primary}
                elapsedMs={working.elapsedMs}
                queued={working.queued}
              />
            )}
            {/*
              卡片浮在画布上：10% 描边加浮层阴影，比错误卡更轻；
              聚焦时补一圈 3px 的聚焦柔光，与设计系统的输入控件同一套反馈。
              拖拽悬停时给落点一圈聚焦色，让用户知道松手会发生什么。
            */}
            <div
              ref={cardRef}
              className={`rounded-[20px] bg-surface hairline shadow-panel transition-colors duration-100 focus-within:border-focus focus-within:shadow-[0_0_0_3px_var(--color-focus-soft)] ${
                dragging ? 'border-focus bg-hover' : 'border-border'
              }`}
            >
              {!compact && (links.links.length > 0 || attachments.length > 0) && (
                <ComposerTray
                  links={links.links}
                  attachments={attachments}
                  // 与消息气泡同一套动作：图片放大查看，文件在右侧栏打开。
                  attachmentOpen={(attachment) =>
                    attachment.image
                      ? () => setPreviewing(attachment.path)
                      : actions === null
                        ? undefined
                        : () => actions.openFile(attachment.path)
                  }
                  onRemoveAttachment={remove}
                  onOpenLink={(url) => {
                    // 打不开时把地址复制到剪贴板，回执落在输入坞的提示行上。
                    void openWebLink(url).then((failure) => {
                      if (failure !== null) notify(failure, 'danger');
                    });
                  }}
                  onRemoveLink={links.remove}
                />
              )}

              <ComposerTextArea
                areaRef={areaRef}
                text={text}
                disabled={!enabled}
                placeholder={
                  !enabled
                    ? PLACEHOLDER_DISABLED
                    : placeholder ?? (occupiesDock ? PLACEHOLDER_OCCUPIED : PLACEHOLDER_READY)
                }
                skillNames={skillNames}
                linkTokens={links.tokens}
                height={LINE_HEIGHT + VERTICAL_PADDING}
                onChange={editing.onChange}
                onCaret={setCaret}
                onPaste={(event) => {
                  // 文件与大段文本照旧由附件控制器接管，剩下的地址粘成占位符。
                  if (handlePaste(event) || compact) return;
                  const area = event.currentTarget;
                  const pasted = event.clipboardData.getData('text');
                  if (
                    links.paste(pasted, {
                      start: area.selectionStart,
                      end: area.selectionEnd,
                    })
                  ) {
                    event.preventDefault();
                  }
                }}
                onCompositionStart={() => {
                  setComposing(true);
                  editing.onCompositionStart();
                }}
                onCompositionEnd={(event) => {
                  setComposing(false);
                  editing.onCompositionEnd(event);
                }}
                onKeyDown={onKeyDown}
                onContextMenu={(event) => menu.open(event, editing.buildMenu())}
                ariaExpanded={openCandidates !== null}
                ariaControls={openCandidates?.listId}
                ariaActiveDescendant={
                  openCandidates === null
                    ? undefined
                    : composerOptionId(openCandidates.listId, openCandidates.index)
                }
              />

              <div className="flex items-center gap-1 px-2.5 pb-2.5">
                {!compact && (
                  <button
                    type="button"
                    aria-label="添加附件"
                    title="添加附件"
                    disabled={!enabled}
                    onClick={() => {
                      setLocalError(null);
                      pick();
                    }}
                    className="shrink-0 w-7 h-7 rounded-sm flex items-center justify-center text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-40"
                  >
                    <PlusIcon size={16} />
                  </button>
                )}

                {/* 键盘提示挂在底部操作行内：贴着添加按钮，不落到卡片外成为脚注。 */}
                {hintVisible && (
                  <span className="shrink-0 pl-1 text-2xs text-text-tertiary">
                    {hint ?? KEYBOARD_HINT}
                  </span>
                )}

                <span className="flex-1" />

                {!compact && (
                  <ComposerPicker
                    model={model}
                    models={models}
                    effort={effort}
                    configBusy={configBusy}
                    configLocked={configLocked}
                    onModelChange={(value) => onModelChange?.(value)}
                    onReasoningEffortChange={(value) => onReasoningEffortChange?.(value)}
                  />
                )}

                <button
                  type="button"
                  aria-label={showStop ? '停止生成' : '发送'}
                  title={
                    showStop
                      ? '停止生成（Esc）'
                      : working !== null
                        ? '发送：Agent 运行中，消息进入排队并在安全边界插入'
                        : '发送'
                  }
                  disabled={showStop ? false : !sendable}
                  onClick={showStop ? onInterrupt : submit}
                  className={`shrink-0 w-8 h-8 rounded-full flex items-center justify-center transition-colors duration-100 ${
                    showStop || sendable
                      ? 'bg-primary text-white'
                      : 'bg-surface-sunken text-text-tertiary opacity-40'
                  } disabled:cursor-not-allowed`}
                >
                  {showStop ? (
                    <StopIcon size={16} />
                  ) : busy ? (
                    <span className="w-3.5 h-3.5 rounded-full border border-current border-t-transparent animate-spin" />
                  ) : (
                    <ArrowUpIcon size={16} />
                  )}
                </button>
              </div>
            </div>
            {/*
              候选菜单是贴着卡片上沿的浮层，因此展开与收起都不改卡片版面。
              两种候选互斥，同一时刻只有一组会渲染。
            */}
            <ComposerCandidateLayer
              anchor={cardRef.current}
              slashListId={SLASH_LIST_ID}
              referenceListId={REFERENCE_LIST_ID}
              slash={slash}
              references={references}
              onAcceptSlash={acceptSlash}
              onAcceptReference={acceptReference}
            />
          </>
        )}
      </div>
      {menu.menu !== null && (
        <ContextMenu items={menu.menu.items} position={menu.menu.position} onClose={menu.close} />
      )}
      {previewing !== null && (
        <ImageLightbox path={previewing} onClose={() => setPreviewing(null)} />
      )}
      {/*
        作答被服务端拒绝用弹窗：它是「这次操作没成」的结论，不是输入坞的常态提示，
        留在输入框上方会与正文里的问答状态打架。
      */}
      <Dialog
        open={questionError !== null}
        onClose={() => onDismissQuestionError?.()}
        title="作答未提交"
        size="sm"
        hint="界面会按服务端状态刷新这条提问。"
        primary={{ label: '知道了', onClick: () => onDismissQuestionError?.() }}
      >
        <p className="whitespace-pre-wrap break-words text-row text-text-secondary">
          {questionError}
        </p>
      </Dialog>
    </div>
  );
};