import React from 'react';
import { structuredInputBlock } from '../../conversation/references';
import { matchBuiltinCommand } from '../../conversation/builtinCommands';
import { openWebLink } from '../../conversation/openWebLink';
import { ComposerTray } from './ComposerTray';
import { useComposerKeyHandler } from './composerKeys';
import { createComposerSubmit } from './composerSubmit';
import { FIELD_SIZING_SUPPORTED } from './composerSizing';
import { useConversationActions } from './actions';
import { ComposerNotices, ComposerWorkingLine } from './ComposerStatus';
import { ImageLightbox } from './ImageLightbox';
import { ComposerCandidateLayer } from './ComposerCandidateLayer';
import { composerOptionId } from './ComposerCandidates';
import { ComposerActionBar } from './ComposerActionBar';
import { ComposerTextArea } from './ComposerTextArea';
import { ComposerVoiceLayer } from './ComposerVoiceLayer';
import { useComposerVoice } from './useComposerVoice';
import { useExpiringNotice } from './composerNotice';
import { useComposerAttachments } from './useComposerAttachments';
import { useComposerEditing, type CaretRange } from './useComposerEditing';
import { useComposerLinks } from './useComposerLinks';
import { useInputHistory } from './useInputHistory';
import { useReferenceCompletion } from './useReferenceCompletion';
import { useSkillNames } from '../../conversation/skillCatalog';
import { useSlashCompletion } from './useSlashCompletion';
import { ContextMenu, useContextMenu } from '../ui/ContextMenu';
import { ComposerQuestionCard } from './ComposerQuestionCard';
import { useQuestionCard } from './useQuestionCard';
import { Dialog } from '../ui/overlays';
import { McpServerPanel } from './McpServerPanel';
import { useComposerCommandRun } from './useComposerCommandRun';
import type { InputDockProps } from './inputDockTypes';

export type { InputDockProps } from './inputDockTypes';

/*
 * 悬浮输入坞。
 *
 * 只接入有真实后端来源的能力：多行输入与提交、模型与推理等级、附件、`@` 文件引用
 * 与 `/` 指令。每块能力各有归属：编辑与撤销历史在 useComposerEditing，附件落盘的三条
 * 路径（粘贴、拖放、系统选择器）在 useComposerAttachments，模型与推理等级在
 * ComposerPicker，两组候选在 useReferenceCompletion 与 useSlashCompletion。
 * 本组件只负责把它们摆到位并处理键盘。
 */

/** 输入框最多向上伸展 6 行；单行 22px、上下内边距合计 20px。 */
const MAX_ROWS = 6;
const LINE_HEIGHT = 22;
const VERTICAL_PADDING = 20;
/** 输入框高度上限，与 `tokens.css` 的 `.composer-input` 同一取值。 */
const MAX_HEIGHT = LINE_HEIGHT * MAX_ROWS + VERTICAL_PADDING;

/* 占位文案：窄列下也要一行放得下；键盘提示只在列宽足够时出现。 */
const PLACEHOLDER_READY = '给 Axon 发送消息，@ 引用文件';
const PLACEHOLDER_OCCUPIED = '请在上方向模型回复，或收起卡片自由输入...';
const PLACEHOLDER_DISABLED = '会话未就绪...';
const KEYBOARD_HINT = 'Shift + Enter 换行';

/** 两组候选菜单的列表 id：输入框用它声明 aria-controls 与 aria-activedescendant。 */
const SLASH_LIST_ID = 'composer-slash-candidates';
const REFERENCE_LIST_ID = 'composer-reference-candidates';

export const InputDock: React.FC<InputDockProps> = ({
  workspaceRoot,
  workspaceId = null,
  replyDestination = '',
  voiceTarget,
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
  onRunCommand,
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
  /*
   * 问答占座卡：何时占位、草稿存在哪、外部入口怎么叫回来，与输入和提交无关，
   * 因此整套状态在 useQuestionCard 里。
   */
  const question = useQuestionCard(activeQuestion, reopenToken);

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
  /** 语音输入：采集、六态与定稿写入都在这个控制器里，紧凑子代理条不接。 */
  const voice = useComposerVoice({
    enabled: !compact && enabled,
    workspaceId,
    replyDestination,
    text,
    commit: editing.commit,
    mode: 'dictation',
    ensureSession: voiceTarget?.ensureSession,
  });
  /** 本机指令（`/mcp`）的执行态、回执与清单：按下即转圈，结束换回执与清单。 */
  const command = useComposerCommandRun(onRunCommand, notify);
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
  /*
   * 语音关闭后把焦点还给输入框：声音停了就该能接着打字。
   * 只在「从非待机变回待机」那一帧聚焦，首屏挂载不会抢焦点。
   */
  const voicePhaseRef = React.useRef(voice.phase);
  React.useEffect(() => {
    const before = voicePhaseRef.current;
    voicePhaseRef.current = voice.phase;
    if (before !== 'idle' && voice.phase === 'idle') areaRef.current?.focus();
  }, [voice.phase]);

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
    // 引擎支持 field-sizing 时高度完全由 CSS 决定：这里连读都不读，
    // 因此打字换行全程零强制同步回流。
    if (FIELD_SIZING_SUPPORTED) return;
    const area = areaRef.current;
    if (area === null) return;
    area.style.height = 'auto';
    area.style.height = `${Math.min(area.scrollHeight, MAX_HEIGHT)}px`;
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

  /**
   * 提交或执行指令之后复位输入坞：草稿、附件、候选、输入历史与编辑历史一起回到初值。
   *
   * 高度同时收回一行，否则空输入框会留着上一次撑开的高度。
   */
  const resetComposer = React.useCallback(() => {
    editing.reset();
    clear();
    links.reset();
    references.reset();
    slash.reset();
    recall.reset();
    setCaret(0);
    if (!FIELD_SIZING_SUPPORTED && areaRef.current !== null) {
      areaRef.current.style.height = `${LINE_HEIGHT + VERTICAL_PADDING}px`;
    }
  }, [editing, clear, links, references, slash, recall]);

  const submit = createComposerSubmit({
    text,
    attachments,
    hasContent,
    busy,
    enabled,
    expandLinks: links.expand,
    buildStructuredInput: (outgoing, paths) =>
      structuredInputBlock(outgoing, references.references, paths),
    runCommand: onRunCommand,
    runLocalCommand: command.run,
    reset: resetComposer,
    recordHistory: (outgoing) => recall.record(outgoing, setLocalError),
    send: onSubmit,
    matchCommand: matchBuiltinCommand,
  });

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

  /*
   * 键盘约定全部交给 composerKeys 里的接线：撤销/恢复、候选选择、历史翻页与提交
   * 只通过这份上下文往来，组件本身不再持有这些分支。
   */
  const onKeyDown = useComposerKeyHandler({
    text,
    caret,
    references,
    slash,
    openCandidates,
    undo: editing.undo,
    redo: editing.redo,
    selectAll: editing.selectAll,
    commitText: editing.commit,
    requestCaret: setPendingCaret,
    previousHistory: recall.previous,
    nextHistory: recall.next,
    submit,
  });

  const effort = reasoningEffort ?? 'medium';

  // 判定是否占座：处于 pending/extended 且未被用户收起时占座；
  // 由外部入口叫回来的卡片同样占座，挂起中的提问只有这一条路径能回到主展示位。
  const occupiesDock = question.visible;

  return (
    /*
     * 正常流式布局，不再绝对定位：输入坞浮在正文之上时，
     * 滚动中的正文会从输入框背面穿过去，边界感只能靠留白假装。
     */
    <div
      {...dropHandlers}
      className={`relative shrink-0 flex justify-center bg-canvas ${
        compact ? 'px-2 pb-2.5 pt-2' : 'px-4 pb-4 pt-3'
      }`}
    >
      <div className="w-full max-w-chat relative">
        {/* 收起走面板内部的 110ms 退场，因此这里必须常驻渲染：条件挂载会让退场无从发生。 */}
        <McpServerPanel servers={command.servers} onClose={command.close} />
        {/* 占座卡：抢占输入坞主展示位 */}
        {occupiesDock && activeQuestion ? (
          <ComposerQuestionCard
            question={activeQuestion}
            busy={busy}
            draft={question.draft}
            onDraftChange={question.setDraft}
            onSubmit={(answers) => onAnswerQuestion?.(activeQuestion.callId, answers)}
            onDismiss={question.dismiss}
          />
        ) : (
          <>
            {!compact && <ComposerVoiceLayer voice={voice} />}
            {/* 本机指令的执行态：按下即出现，结束后换成回执与清单。 */}
            {command.label !== null && (
              <ComposerWorkingLine primary={command.label} elapsedMs={command.elapsedMs} queued={0} interruptible={false} />
            )}
            <ComposerNotices error={error} localError={localError} notice={notice} />
            {/* 工作状态行：与 TUI 的 composer dock 同位置同口径，运行期间一直保留。 */}
            {working !== null && (
              <ComposerWorkingLine
                primary={working.primary}
                elapsedMs={working.elapsedMs}
                queued={working.queued}
              />
            )}
            {/*
              双层同心倒角输入坞（Double-Bezel Architecture）：
              外层采用 3px 极轻透微下沉托槽，运行期间叠加柔和呼吸微光；
              内层是 18px 圆角的纯白浮起核心，聚焦态保持立体投影绝不塌陷。
            */}
            <div className="dock-shell relative p-[3px] rounded-[21px] bg-surface-sunken/45 hairline border-border-hairline">
              {/* 聚焦呼吸贴在托槽边框本身：inset-0 加同心 21px 圆角，既不内缩也不外扩。 */}
              <span
                aria-hidden="true"
                className="dock-focus-frame pointer-events-none absolute inset-0 z-20 rounded-[21px]"
              />
              {(working !== null || command.running !== null) && (
                <span
                  aria-hidden="true"
                  className="dock-ambient-glow pointer-events-none absolute inset-0 rounded-[21px] hairline border-focus/45"
                />
              )}
              <div
                ref={cardRef}
                className={`relative rounded-[18px] bg-surface hairline shadow-dock transition-colors duration-100 ${
                  dragging ? 'border-focus bg-hover' : 'border-border'
                }`}
              >
                {!compact && (links.links.length > 0 || attachments.length > 0) && (
                  <ComposerTray
                    links={links.links}
                    attachments={attachments}
                    attachmentOpen={(attachment) =>
                      attachment.image
                        ? () => setPreviewing(attachment.path)
                        : actions === null
                          ? undefined
                          : () => actions.openFile(attachment.path)
                    }
                    onRemoveAttachment={remove}
                    onOpenLink={(url) => {
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

                <ComposerActionBar
                  compact={compact}
                  enabled={enabled}
                  hintVisible={hintVisible}
                  hintText={hint ?? KEYBOARD_HINT}
                  model={model}
                  models={models}
                  effort={effort}
                  configBusy={configBusy}
                  configLocked={configLocked}
                  showStop={showStop}
                  sendable={sendable}
                  busy={busy}
                  commandRunning={command.running !== null}
                  hasContent={hasContent}
                  working={working !== null}
                  voiceAvailable={voice.available}
                  voiceActive={voice.active}
                  onToggleVoice={voice.toggle}
                  onPickAttachment={() => {
                    setLocalError(null);
                    pick();
                  }}
                  onModelChange={onModelChange}
                  onReasoningEffortChange={onReasoningEffortChange}
                  onInterrupt={onInterrupt}
                  onSubmit={submit}
                />
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
