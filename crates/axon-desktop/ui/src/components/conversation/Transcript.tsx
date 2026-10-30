import React from 'react';
import {
  AlertCircleIcon,
  ApiIcon,
  BrainIcon,
  CheckIcon,
  ChecklistIcon,
  CopyIcon,
  EditIcon,
  FileIcon,
  RetryIcon,
  SearchIcon,
  SkillIcon,
  SpinnerIcon,
  TerminalIcon,
  ToolIcon,
} from '../../icons';
import {
  compactDuration,
  LOCAL_STEER_PREFIX,
  type AttachmentRef,
  type TranscriptCell,
} from '../../conversation/transcript';
import * as ipc from '../../ipc';
import { Text } from '../ui/surfaces';
import { useConversationActions } from './actions';
import { AssistantBlock } from './AssistantBlock';
import { AttachmentTile } from './AttachmentTile';
import { BUBBLE_RADIUS } from './chatChrome';
import { DiffStat, DisclosureBody, DisclosureRow } from './DisclosureRow';
import { FailureCard } from './FailureCard';
import { ImageLightbox } from './ImageLightbox';
import { MessageActionButton, MessageFooter } from './MessageFooter';
import { QuestionCellBlock } from './QuestionCellBlock';
import { SummaryBlock } from './SummaryBlock';
import { ToolContent } from './ToolContent';
import { UserText } from './UserText';
import { extractFilePath, toolFailed, toolLabel, toolRowTitle } from './toolPresentation';

/*
 * 时间线单元的呈现。
 *
 * 可展开的行统一交给 DisclosureRow：行高 28px、左侧 56px 标签列、箭头 12px
 * 且只有「收起朝右、展开朝下」一个方向。此前每一类行各写一套箭头与行高，
 * 同一屏里并排出现时读起来不是一套东西。
 *
 * 思考行的单行内容只是收起态的预览：展开后正文已完整落在下方，行头随即去掉标题，
 * 避免同一段思考在行头与正文里各说一遍（与既有工作台产品的思考行同形态）。
 */

/** 用户消息：右侧不对称胶囊；悬浮或键盘聚焦时在气泡下方浮出操作条。 */
/**
 * 附件卡：与输入区同款（类型徽标 + 文件名 + 类别与体量）。
 *
 * 图片由渲染层按地址直接取字节：不再把 base64 读进 JS 内存，一张 1.5 MB 的截图
 * 编码后会变成约 2 MB 的字符串常驻，而按地址取图既省掉这份开销，也让 webview
 * 自己决定缓存与解码时机。点卡片放大查看。
 */
const AttachmentCard: React.FC<{ attachment: AttachmentRef; onOpen?: () => void }> = ({
  attachment,
  onOpen,
}) => <AttachmentTile attachment={attachment} onOpen={onOpen} />;

/** 用户消息：附件在上，气泡在下；时间常显，操作按钮随悬浮出现。 */
const UserBubble: React.FC<{
  text: string;
  atMs: number | null;
  attachments: AttachmentRef[];
  /** 仍在等待后端在安全边界插入：本地标识还没被 durable 事件认领。 */
  pending?: boolean;
  /** 已交给中断转投队列：气泡原地留着，等下一轮接手。 */
  carrying?: boolean;
  /** 服务端始终没有认领这条消息；内容还在，可直接重新发送。 */
  undelivered?: boolean;
}> = ({
  text,
  atMs,
  attachments,
  pending = false,
  carrying = false,
  undelivered = false,
}) => {
  const actions = useConversationActions();
  const [copied, setCopied] = React.useState(false);
  /** 剪贴板不可用时如实标记，不谎报复制成功。 */
  const [copyFailed, setCopyFailed] = React.useState(false);
  const [previewing, setPreviewing] = React.useState<string | null>(null);
  return (
    <div className="group flex flex-col items-end">
      {/* 排队态是这一条消息自己的事实：durable 事件认领它之后这个标记自行消失。 */}
      {pending && (
        <span className="mb-1 inline-flex items-center gap-1 text-meta text-text-tertiary">
          <SpinnerIcon size={12} className="state-spinner" />
          等待注入
        </span>
      )}
      {/* 转投中：中断时交给下一轮，气泡留在原处，不再算本轮等待注入。 */}
      {carrying && (
        <span className="mb-1 inline-flex items-center gap-1 text-meta text-text-tertiary">
          转投下一条
        </span>
      )}
      {/* 未送达同样是这一格自己的事实：轮次收口或提交失败后服务端仍未认领它。 */}
      {undelivered && (
        <span className="mb-1 inline-flex items-center gap-1 text-meta text-gold-ink">
          <AlertCircleIcon size={12} />
          未送达
        </span>
      )}
      {attachments.length > 0 && (
        <div className="mb-1.5 flex flex-wrap justify-end gap-1.5 max-w-[75%]">
          {attachments.map((attachment) => (
            <AttachmentCard
              key={attachment.path}
              attachment={attachment}
              // 图片点开放大查看；文件在右侧栏打开（没有会话动作时不可点）。
              onOpen={
                attachment.image
                  ? () => setPreviewing(attachment.path)
                  : actions === null
                    ? undefined
                    : () => actions.openFile(attachment.path)
              }
            />
          ))}
        </div>
      )}
      {text.trim().length > 0 && (
        <div
          className="max-w-[75%] px-4 py-2.5 bg-bubble hairline border-border-hairline shadow-card text-content text-text-primary whitespace-pre-wrap break-words"
          style={{ borderRadius: BUBBLE_RADIUS }}
        >
          {/* 叫技能的 `/name` 在这里也点亮，网页地址收敛成一枚可打开的链接芯片。 */}
          <UserText text={text} />
        </div>
      )}
      <MessageFooter atMs={atMs} align="end">
        <MessageActionButton
          label="复制消息"
          title="复制消息"
          onClick={() => {
            // 复制走原生剪贴板：网页剪贴板要求窗口处于焦点，复制会时灵时不灵。
            void ipc.clipboardWriteText(text).then(
              () => {
                setCopied(true);
                window.setTimeout(() => setCopied(false), 1500);
              },
              () => setCopyFailed(true),
            );
          }}
        >
          {copyFailed ? (
            <AlertCircleIcon size={14} className="text-danger" />
          ) : copied ? (
            <CheckIcon size={14} className="text-ok-ink" />
          ) : (
            <CopyIcon size={14} />
          )}
        </MessageActionButton>
        {actions !== null && (
          <MessageActionButton
            label="重新发送"
            title="重新发送这条消息"
            onClick={() => actions.retry(text, attachments)}
          >
            <RetryIcon size={14} />
          </MessageActionButton>
        )}
      </MessageFooter>
      {previewing !== null && (
        <ImageLightbox path={previewing} onClose={() => setPreviewing(null)} />
      )}
    </div>
  );
};

/** 思考块。 */
/**
 * 思考正文的紧凑化：把空行折成单个换行。
 *
 * 思考是过程日志而不是排版文章，模型给的段落之间常夹着空行；照原样渲染会在
 * 灰底块里留出成片空白，读起来像断了一大截。折行后行与行连续，密度与工具行一致。
 */
function compactReasoningText(text: string): string {
  return text.replace(/\n{2,}/g, '\n');
}

const ReasoningBlock: React.FC<{ text: string; done: boolean; durationMs: number | null }> = ({
  text,
  done,
  durationMs,
}) => {
  const [open, setOpen] = React.useState(!done);
  const wasRunningRef = React.useRef(!done);
  React.useEffect(() => {
    if (!done) {
      wasRunningRef.current = true;
      setOpen(true);
      return undefined;
    }
    if (!wasRunningRef.current) return undefined;
    wasRunningRef.current = false;
    // 思考刚收口时保留 180ms 视觉缓冲再自动折叠，避免正文突然上跳。
    const timer = window.setTimeout(() => setOpen(false), 180);
    return () => window.clearTimeout(timer);
  }, [done]);

  /*
   * 思考正文的贴底跟随。
   *
   * 限高容器一旦出现滚动条，新吐出的思考只会追加在底部而视口停在顶上，
   * 读者得不停手动下滑。这里在每次文本增长后把视口贴到底部，让正在生成的
   * 那一段始终可见；判定放在布局阶段，量与写发生在同一帧，不会先闪一下顶部再跳到底。
   *
   * 只跟随「还没收口」的思考：收口后再写入就是历史正文，不能替读者挪动阅读位置。
   * 读者主动上滑时让出跟随权，滑回底部再自动恢复，与正文区的贴底规则同一口径。
   */
  const bodyRef = React.useRef<HTMLDivElement>(null);
  const pinnedRef = React.useRef(true);
  React.useLayoutEffect(() => {
    if (!open || done) return;
    const body = bodyRef.current;
    if (body === null) return;
    if (pinnedRef.current) body.scrollTop = body.scrollHeight;
  }, [text, open, done]);

  const followLatest = React.useCallback(() => {
    const body = bodyRef.current;
    if (body === null) return;
    // 距底 24px 以内算贴底：留出亚像素舍入与行高抖动的余量。
    pinnedRef.current = body.scrollHeight - body.scrollTop - body.clientHeight <= 24;
  }, []);

  const firstLine = text.trim().split('\n')[0] ?? '';
  // 收起态拿首行当单行预览；展开后正文已完整呈现，行头不再重复这段内容。
  const preview = open || firstLine.length === 0 ? undefined : firstLine;
  return (
    <div className="w-full">
      <DisclosureRow
        open={open}
        onToggle={() => setOpen((value) => !value)}
        active={!done}
        icon={<BrainIcon size={14} />}
        label="思考"
        title={preview}
        titleText={firstLine}
        meta={compactDuration(durationMs)}
      />
      {open && (
        <DisclosureBody framed>
          {/*
            思考是过程批注：限高内部滚动避免长篇思考撑爆整屏，左侧细标线加柔和次级墨色。
          */}
          <div
            ref={bodyRef}
            onScroll={followLatest}
            className="max-h-[320px] overflow-y-auto scroll-visible border-l-[1.5px] border-border-strong pl-3.5 pr-2 py-0.5"
          >
            <p className="text-row-loose text-text-secondary whitespace-pre-wrap break-words">
              {compactReasoningText(text)}
            </p>
          </div>
        </DisclosureBody>
      )}
    </div>
  );
};

function toolIcon(label: string): React.ReactNode {
  switch (label) {
    case '读取':
    case '读取便签':
      return <FileIcon size={14} />;
    case '检索':
    case '匹配':
    case '记忆检索':
    case '检索便签':
    case '匹配便签':
      return <SearchIcon size={14} />;
    case '编辑':
    case '写入':
    case '记忆写入':
    case '便签':
    case '写入便签':
      return <EditIcon size={14} />;
    case '执行':
      return <TerminalIcon size={14} />;
    case 'MCP':
      return <ApiIcon size={14} />;
    case '技能':
      return <SkillIcon size={14} />;
    default:
      return <ToolIcon size={14} />;
  }
}

/** 工具块：类别前缀 + 命令或路径，行尾给退出码、耗时或增删行数。 */
const ToolBlock: React.FC<{ cell: Extract<TranscriptCell, { role: 'tool' }> }> = ({ cell }) => {
  const running =
    cell.status === 'running' || cell.status === 'started' || cell.status === 'waiting';
  const [open, setOpen] = React.useState(false);
  const actions = useConversationActions();
  const label = toolLabel(cell.toolKind, cell.toolName);
  /*
   * 折叠态的主文本由 toolPresentation 按工具语义给出：模型自述目的优先，其次是这一行
   * 最该看见的那一项参数（记忆检索的 query、技能的技能名、子代理的 task 等）。
   * 返回 null 表示收起时不追加内容（例如查看剩余上下文，参数本身没有信息量）。
   * 命令与原始 JSON 不丢：展开后才显示输入、变更与输出。
   */
  const primary = toolRowTitle(cell);
  const failed = toolFailed(cell.status, cell.exitCode);
  // 读写类工具的主文本就是工作区内的相对路径：点它直接在右侧栏打开该文件。
  const openablePath =
    actions !== null && (label === '读取' || label === '编辑' || label === '写入') && cell.editPath !== null
      ? cell.editPath
      : actions !== null && (label === '读取' || label === '编辑' || label === '写入')
        ? extractFilePath(cell.arguments) ?? extractFilePath(cell.command)
        : null;
  const timing = [cell.exitCode !== null ? `exit ${cell.exitCode}` : null, compactDuration(cell.durationMs)]
    .filter((value): value is string => value !== null && value.length > 0)
    .join(' · ');
  const meta =
    cell.added > 0 || cell.removed > 0 ? (
      <DiffStat added={cell.added} removed={cell.removed} />
    ) : timing.length > 0 ? (
      timing
    ) : undefined;
  return (
    <div className="w-full">
      <DisclosureRow
        open={open}
        onToggle={() => setOpen((value) => !value)}
        active={running}
        icon={toolIcon(label)}
        label={label}
        danger={failed}
        titleText={primary ?? label}
        title={
          primary === null ? undefined : openablePath === null ? (
            primary
          ) : (
            <span
              role="link"
              tabIndex={0}
              title={`在右侧栏打开 ${openablePath}`}
              onClick={(event) => {
                event.stopPropagation();
                actions?.openFile(openablePath);
              }}
              onKeyDown={(event) => {
                if (event.key !== 'Enter' && event.key !== ' ') return;
                event.preventDefault();
                event.stopPropagation();
                actions?.openFile(openablePath);
              }}
              className="underline decoration-dotted underline-offset-2 transition-colors duration-100 hover:text-text-primary"
            >
              {primary}
            </span>
          )
        }
        meta={meta}
      />
      {open && (
        <DisclosureBody>
          {cell.toolKind !== 'notes' && (cell.arguments.length > 0 || cell.command.length > 0) && (
            <ToolContent title="输入" value={cell.arguments || cell.command}
              shell={label === '执行'} />
          )}
          {cell.toolKind !== 'notes' && cell.summary.length > 0 && cell.summary !== cell.output && (
            <ToolContent title="摘要" value={cell.summary} />
          )}
          {cell.diff !== null && cell.diff.length > 0 && (
            <ToolContent title="变更" value={cell.diff} language="diff" />
          )}
          {cell.output.length > 0 && (
            <ToolContent title="输出" value={cell.output} />
          )}
        </DisclosureBody>
      )}
    </div>
  );
};

/** 独立的文件变更行：只有无法认领到工具调用时才出现。 */
const FileChangeBlock: React.FC<{ cell: Extract<TranscriptCell, { role: 'fileChange' }> }> = ({
  cell,
}) => {
  const [open, setOpen] = React.useState(false);
  return (
    <div className="w-full">
      <DisclosureRow
        open={open}
        onToggle={() => setOpen((value) => !value)}
        icon={<EditIcon size={14} />}
        label="编辑"
        title={cell.path}
        titleText={cell.path}
        meta={<DiffStat added={cell.added} removed={cell.removed} />}
      />
      {open && cell.diff.length > 0 && (
        <DisclosureBody>
          <pre className="my-1 px-3 py-2 rounded-md bg-surface-sunken hairline border-border-hairline max-h-[320px] overflow-auto font-mono text-code text-text-secondary whitespace-pre-wrap break-words">
            {cell.diff}
          </pre>
        </DisclosureBody>
      )}
    </div>
  );
};

/** 压缩（窗口轮转）行。 */
const HistoryBlock: React.FC<{ cell: Extract<TranscriptCell, { role: 'history' }> }> = ({
  cell,
}) => {
  const [open, setOpen] = React.useState(false);
  return (
    <div className="w-full">
      <DisclosureRow
        open={open}
        onToggle={() => setOpen((value) => !value)}
        icon={<ChecklistIcon size={14} />}
        label="历史"
        title={cell.summary}
        titleText={cell.summary}
        meta={cell.status}
      />
      {open && (
        <DisclosureBody>
          <Text tone="secondary" size="meta" className="block break-words">
            {cell.title} · {cell.summary}
          </Text>
        </DisclosureBody>
      )}
    </div>
  );
};

/**
 * 提示与错误行：内容本身就是结论，不需要展开。
 *
 * 左缘留出与可展开行标题列同宽的空白：提示因此不会比过程行更靠左，
 * 整段正文的左缘仍是一条竖线。
 */
const NoticeBlock: React.FC<{ cell: Extract<TranscriptCell, { role: 'notice' }> }> = ({ cell }) => (
  <div className="w-full flex items-start gap-1.5 py-1 pl-[102px]">
    <Text
      size="meta"
      strong
      tone={cell.tone === 'danger' ? 'danger' : 'primary'}
      className="shrink-0"
    >
      {cell.title}
    </Text>
    <span className="shrink-0 text-text-tertiary">·</span>
    <Text tone="secondary" size="meta" className="min-w-0 break-words">
      {cell.summary}
    </Text>
  </div>
);

/**
 * 折叠行：一整条 Interaction 压成的一行。
 *
 * 折叠时正文已经释放，因此这一行本身不展开任何内容，只负责把人带回那条记录：
 * 点击即按标识重新回放，回放完把正文整段换回来。
 */
const FoldedBlock: React.FC<{ cell: Extract<TranscriptCell, { role: 'folded' }> }> = ({ cell }) => {
  const actions = useConversationActions();
  const label = cell.prompt.trim().length > 0 ? cell.prompt : '这一轮的历史记录';
  return (
    <div className="w-full rounded-md hairline border-border-hairline bg-surface px-1 py-0.5">
      <DisclosureRow
        open={false}
        onToggle={() => actions?.unfoldTurn(cell.interactionId)}
        icon={<ChecklistIcon size={14} />}
        label="历史"
        title={label}
        titleText={label}
        meta={cell.summary}
      />
    </div>
  );
};

/** 单个单元的渲染入口。 */
export const TranscriptCellView: React.FC<{ cell: TranscriptCell }> = React.memo(({ cell }) => {
  switch (cell.role) {
    case 'user':
      return (
        <UserBubble
          text={cell.text}
          atMs={cell.atMs}
          attachments={cell.attachments}
          pending={
            cell.id.startsWith(LOCAL_STEER_PREFIX) &&
            cell.undelivered !== true &&
            cell.requeued !== true
          }
          carrying={cell.requeued === true && cell.undelivered !== true}
          undelivered={cell.undelivered === true}
        />
      );
    case 'assistant':
      return (
        <AssistantBlock
          text={cell.text}
          streaming={cell.streaming}
          atMs={cell.atMs}
          source={cell.id}
        />
      );
    case 'reasoning':
      return <ReasoningBlock text={cell.text} done={cell.done} durationMs={cell.durationMs} />;
    case 'tool':
      return <ToolBlock cell={cell} />;
    case 'fileChange':
      return <FileChangeBlock cell={cell} />;
    case 'history':
      return <HistoryBlock cell={cell} />;
    case 'plan':
      /*
       * 计划已由输入坞上方的固定计划坞 (PlanDock) 承担，正文渲染区不再重复渲染。
       */
      return null;
    case 'folded':
      return <FoldedBlock cell={cell} />;
    case 'question':
      return <QuestionCellBlock cell={cell} />;
    case 'notice':
      return <NoticeBlock cell={cell} />;
    case 'failure':
      return cell.report === null ? (
        <NoticeBlock cell={{ id: cell.id, role: 'notice', title: '轮次失败', summary: cell.text, tone: 'danger' }} />
      ) : (
        <FailureCard report={cell.report} fallbackText={cell.text} />
      );
    case 'summary':
      return <SummaryBlock cell={cell} />;
    default:
      return null;
  }
});
