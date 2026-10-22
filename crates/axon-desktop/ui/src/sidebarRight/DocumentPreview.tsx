import React from 'react';
import * as ipc from '../ipc';
import { CheckIcon, FileIcon, NoWrapIcon, RefreshIcon, WrapIcon } from '../icons';
import { Dropdown } from '../components/ui/overlays';
import { EmptyState, LoadingState } from '../components/ui/surfaces';
import { OpenInAppAction } from './OpenInAppAction';
import { ViewerOutlet } from './document';
import { FileTree } from '../components/conversation/fileTree';
import { PathLabel, ToolButton } from './document/parts';
import type { ViewerContent } from './document/contract';
import { viewersFor } from './document/viewers';

/*
 * 文件查看页：右侧栏里每个文件一个 tab 的正文。
 *
 * 排版按停靠栏的统一几何摆：36px 工具栏（路径、呈现方式、换行、重读、
 * 用其他应用打开），下面交给**当前看法自己的正文**（BodyShell 再统一给 36px 头与一个滚动区，
 * 因此正文起点恒为 72）。所有者只做四件事：挑候选、按候选的读取方式取内容、画工具栏、
 * 把内容交给渲染器；滚动区、内边距、缩放这些「属于某一种看法」的东西都在渲染器里。
 *
 * 呈现方式是**跟 tab 走的**：用户选过之后写进布局，切 tab、收起面板甚至重启都还在，
 * 不会在他切回来时莫名其妙地变回默认。
 */

export interface DocumentPreviewProps {
  /** 工作区根目录绝对路径。 */
  root: string;
  /** 待预览的文件相对路径。 */
  path: string;
  /** 需要定位到的行号；为空则从顶部开始。 */
  line?: number;
  /** 这个 tab 上记着的呈现方式；为空时取第一个候选。 */
  viewer?: string;
  /** 用户切换呈现方式时回写布局。 */
  onViewer: (viewer: string) => void;
  /** 从目录树里打开一个文件：与文件页同一套入口，新开一个 tab。 */
  onOpenFile: (path: string) => void;
}

export const DocumentPreview: React.FC<DocumentPreviewProps> = ({
  root,
  path,
  line,
  viewer,
  onViewer,
  onOpenFile,
}) => {
  const candidates = React.useMemo(() => viewersFor(path), [path]);
  const definition = candidates.find((candidate) => candidate.id === viewer) ?? candidates[0];
  const [text, setText] = React.useState<{ text: string; truncated: boolean } | null>(null);
  const [bytes, setBytes] = React.useState<{
    mime: string;
    base64: string;
    truncated: boolean;
  } | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [notice, setNotice] = React.useState<string | null>(null);
  const [revision, setRevision] = React.useState(0);
  const [wrap, setWrap] = React.useState(true);
  /** 后端给出的绝对路径；读出来之前路径栏先显示相对路径。 */
  const [absolute, setAbsolute] = React.useState<string | null>(null);
  /**
   * 这个地址指向目录时，按目录树展开而不是当文本读。
   *
   * 正文里的 `~\.qoder` 这类路径是目录：按文本读只会得到「目标不是文件」，
   * 用户要看的是里面有什么。
   */
  const [directory, setDirectory] = React.useState<{ root: string; display: string } | null>(null);

  const loading = definition?.loading ?? 'none';
  const viewerId = definition?.id ?? 'plain';

  React.useEffect(() => {
    let cancelled = false;
    setText(null);
    setBytes(null);
    setError(null);
    setNotice(null);
    setAbsolute(null);
    setDirectory(null);
    // 先问一句「这是目录吗」：目录不进读取流程，直接换成目录树。
    void ipc
      .statWorkspacePath(root, path)
      .then((stat) => {
        if (cancelled || !stat.is_dir) return;
        setDirectory({ root: stat.absolute_path, display: stat.display_path });
      })
      .catch(() => {
        // 探测失败不必单独报错：下面的读取流程会给出真实原因。
      });
    if (loading === 'none') return undefined;
    if (loading === 'text') {
      void ipc
        .readWorkspaceFile(root, path)
        .then((value) => {
          if (cancelled) return;
          // 二进制文件按文本读只会得到一屏乱码：如实说明，并保留「用其他应用打开」的出路。
          if (value.binary) {
            setError('该格式文件暂时无法预览');
            return;
          }
          setAbsolute(value.absolute_path);
          setText({ text: value.content, truncated: value.truncated });
        })
        .catch((failure: unknown) => {
          if (!cancelled) setError(failure instanceof Error ? failure.message : '读取文件失败');
        });
      return () => {
        cancelled = true;
      };
    }
    void ipc
      .readWorkspaceBytes(root, path)
      .then((value) => {
        if (!cancelled) {
          setAbsolute(value.absolute_path);
          setBytes({ mime: value.mime, base64: value.data_base64, truncated: value.truncated });
        }
      })
      .catch((failure: unknown) => {
        if (!cancelled) setError(failure instanceof Error ? failure.message : '读取文件失败');
      });
    return () => {
      cancelled = true;
    };
  }, [root, path, loading, revision]);

  const content: ViewerContent | null =
    bytes !== null
      ? { kind: 'bytes', mime: bytes.mime, base64: bytes.base64, truncated: bytes.truncated }
      : text !== null
        ? { kind: 'text', text: text.text, truncated: text.truncated }
        : null;

  /** 路径栏显示绝对路径；还没读到时先显示相对路径，避免空着。 */
  const displayPath = directory?.display ?? absolute ?? path;

  const toolbar = (
    <div className="shrink-0 h-9 pl-4 pr-1.5 flex items-center gap-1 hairline-b border-border-hairline">
      <PathLabel path={displayPath} />
      {candidates.length > 1 && directory === null && (
        <Dropdown
          align="end"
          className="shrink-0"
          trigger={({ toggle }) => (
            <button
              type="button"
              aria-label="呈现方式"
              title={definition?.title ?? '纯文本'}
              onClick={toggle}
              className="shrink-0 max-w-[160px] px-1.5 py-1 truncate rounded-sm text-meta text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary"
            >
              {definition?.title ?? '纯文本'}
            </button>
          )}
        >
          {candidates.map((candidate) => (
            <button
              key={candidate.id}
              type="button"
              role="menuitem"
              onClick={() => onViewer(candidate.id)}
              className={`w-full min-h-[30px] py-1 px-2 flex items-center gap-1.5 rounded-md text-left text-row leading-5 text-text-primary transition-colors duration-100 hover:bg-hover ${
                candidate.id === viewerId ? 'bg-hover' : ''
              }`}
            >
              <span className="flex-1 min-w-0 truncate">{candidate.title}</span>
              {candidate.id === viewerId && <CheckIcon size={13} className="shrink-0" />}
            </button>
          ))}
        </Dropdown>
      )}
      {definition?.wrap === true && (
        <ToolButton
          label={wrap ? '取消换行' : '自动换行'}
          pressed={wrap}
          disabled={content === null}
          onClick={() => setWrap((value) => !value)}
        >
          {wrap ? <NoWrapIcon size={14} /> : <WrapIcon size={14} />}
        </ToolButton>
      )}
      <ToolButton
        label="重新读取文件"
        disabled={loading === 'none'}
        onClick={() => setRevision((value) => value + 1)}
      >
        <RefreshIcon size={14} />
      </ToolButton>
      <OpenInAppAction root={root} path={path} onNotice={setNotice} />
    </div>
  );

  const body = (): React.ReactNode => {
    // 目录：按目录树展开，点击里面的文件照常新开预览 tab。
    if (directory !== null) {
      return (
        <div className="flex-1 min-h-0 overflow-y-auto scroll-visible py-1">
          <FileTree root={directory.root} activePath={null} onOpenFile={onOpenFile} />
        </div>
      );
    }
    /*
     * 一份文件连一种看法都没有：不读内容，直接把「看不了」与出路摆在中间。
     * 这里刻意**不发读取请求**：打开一个几百兆的视频不该先把字节读进内存。
     */
    if (definition === undefined) {
      return (
        <UnpreviewableBody root={root} path={path} message="该格式文件暂时无法预览" />
      );
    }
    if (error !== null) {
      return <UnpreviewableBody root={root} path={path} message={error} />;
    }
    if (content === null && loading !== 'none') {
      return (
        <div className="flex-1 min-h-0 flex items-center justify-center">
          <LoadingState label="文档渲染中" />
        </div>
      );
    }
    return (
      <ViewerOutlet
        viewer={viewerId}
        root={root}
        path={path}
        content={content ?? { kind: 'text', text: '', truncated: false }}
        wrap={definition.wrap && wrap}
        line={line}
      />
    );
  };

  return (
    <div className="flex-1 min-h-0 flex flex-col">
      {toolbar}
      {notice !== null && (
        <div className="shrink-0 flex items-center gap-2.5 px-4 py-1.5">
          <span className="break-words text-meta text-danger">{notice}</span>
        </div>
      )}
      {body()}
    </div>
  );
};

/** 看不了时的空态：类型图形 + 一句说明 + 「用其他应用打开」。 */
const UnpreviewableBody: React.FC<{ root: string; path: string; message: string }> = ({
  root,
  path,
  message,
}) => (
  <div className="flex-1 min-h-0 flex items-center justify-center">
    <EmptyState
      icon={<FileIcon size={24} />}
      title="暂时无法预览"
      description={message}
      action={<OpenInAppAction root={root} path={path} prominent />}
    />
  </div>
);

