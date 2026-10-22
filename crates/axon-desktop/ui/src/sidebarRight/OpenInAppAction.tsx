import React from 'react';
import * as ipc from '../ipc';
import type { FileApplicationItem } from '../types';
import { ChevronDownIcon, FolderOpenIcon, OpenInAppIcon } from '../icons';
import { Dropdown } from '../components/ui/overlays';

/*
 * 「用其他应用打开」：一枚分裂按钮加一张下拉菜单。
 *
 * 候选不是一份精选目录，而是**系统自己登记的关联**：菜单在每次展开时重新问一次后端，
 * 因此装上或卸掉一个应用，菜单自己会变。主按钮打开的就是菜单里标着「（默认）」的那一条：
 * 两者永远指同一个应用，不存在「按钮说 A、菜单说 B」。
 *
 * 一个应用都没查到（或查询失败，此时菜单里会多一条说明行）时，主按钮退化为「显示文件位置」：
 * 文件定位始终可用，它不会因为关联查询出问题而消失。
 */

/** 菜单卡片外观：与全站浮层同一套描边与柔光，圆角取浮层档。 */
const MENU_CARD = 'visible bg-surface rounded-lg shadow-menu hairline border-border-hairline';

/** 菜单行的共有外观：紧凑一行，图标位固定。 */
const MENU_ITEM =
  'w-full min-h-[30px] py-1 px-2 flex items-center gap-1.5 rounded-md text-left text-row leading-5 transition-colors duration-100';

/** 主按钮与箭头按钮共有的外观。 */
const SPLIT_HALF =
  'inline-flex items-center justify-center border-0 bg-transparent text-text-primary text-code leading-4 whitespace-nowrap cursor-pointer transition-colors duration-100 hover:bg-hover disabled:cursor-default';

/** 应用图标：取不到图标或图片加载失败时退回通用「外部打开」图形。 */
const ApplicationIcon: React.FC<{ source: string | null; size?: number }> = ({
  source,
  size = 14,
}) => {
  const [failed, setFailed] = React.useState(false);
  if (source === null || failed) return <OpenInAppIcon size={size} />;
  return (
    <img
      src={source}
      width={size}
      height={size}
      alt=""
      draggable={false}
      className="object-contain"
      onError={() => setFailed(true)}
    />
  );
};

export interface OpenInAppActionProps {
  /** 工作区根目录绝对路径。 */
  root: string;
  /** 待打开的文件相对路径。 */
  path: string;
  /** 强调形态：36px 高、带文字的按钮，用于「这份文件预览不了」的空态。 */
  prominent?: boolean;
  /** 把失败信息交给外层正文显示；工具栏自己没有多余的一行。 */
  onNotice?: (message: string | null) => void;
}

export const OpenInAppAction: React.FC<OpenInAppActionProps> = ({
  root,
  path,
  prominent = false,
  onNotice,
}) => {
  const [applications, setApplications] = React.useState<FileApplicationItem[] | null>(null);
  const [failed, setFailed] = React.useState(false);
  const [revision, setRevision] = React.useState(0);
  const [pending, setPending] = React.useState(false);
  const inFlight = React.useRef(false);

  // 关联查询按「文件」走：换一个文件就重新问一次，不沿用上一个文件的候选。
  React.useEffect(() => {
    let cancelled = false;
    setApplications(null);
    setFailed(false);
    void ipc
      .fileApplications(root, path)
      .then((value) => {
        if (!cancelled) setApplications(value);
      })
      .catch(() => {
        if (!cancelled) {
          setApplications([]);
          setFailed(true);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [root, path, revision]);

  /** 每次展开菜单都重问一次：系统里刚装的应用要立刻能选。 */
  const refresh = React.useCallback(() => setRevision((value) => value + 1), []);

  /*
   * 展开回调必须是稳定的：浮层把它当成 effect 依赖，每次渲染都换一个新函数
   * 会让那个 effect 每个渲染都跑一遍，于是「展开就重查」变成「一直重查」。
   */
  const handleOpenChange = React.useCallback(
    (open: boolean) => {
      if (open) refresh();
    },
    [refresh],
  );

  const apps = applications ?? [];
  const defaultId = apps.find((app) => app.default)?.id;
  // 主按钮要打开的那一条：系统标了默认就用它，否则用列表第一条（Shell 自己的偏好顺序）。
  const preferred = apps.find((app) => app.id === defaultId) ?? apps[0];
  const loading = applications === null;
  const revealDefault = !loading && preferred === undefined;
  const disabled = pending;
  // 有应用就有「应用 + 显示文件位置」两条，菜单因此始终存在；一条都没有时只剩定位，不画箭头。
  const hasMenu = loading || failed || apps.length >= 1;
  const primaryLabel =
    preferred === undefined ? '显示文件位置' : `用 ${preferred.name} 打开`;
  // 失败时把定位提升为默认动作：查询出问题不该把「找到这个文件」也一起挡掉。
  const footerLabel = revealDefault ? '显示文件位置（默认）' : '显示文件位置';

  const run = React.useCallback(
    (action: Promise<void>) => {
      if (inFlight.current) return;
      inFlight.current = true;
      setPending(true);
      onNotice?.(null);
      void action
        .catch((failure: unknown) => {
          onNotice?.(failure instanceof Error ? failure.message : '操作失败');
        })
        .finally(() => {
          inFlight.current = false;
          setPending(false);
        });
    },
    [onNotice],
  );

  const primary = () => {
    if (revealDefault) {
      run(ipc.revealWorkspaceFile(root, path));
      return;
    }
    // 系统没标默认时把列表第一条**显式**报上去，而不是让系统再解析一次关联。
    if (preferred !== undefined && preferred.id !== defaultId) {
      run(ipc.openWorkspaceFileWith(root, path, preferred.id));
      return;
    }
    run(ipc.openWorkspaceFile(root, path));
  };

  const icon = loading && preferred === undefined ? (
    <span
      aria-hidden="true"
      data-open-target-skeleton
      className="inline-block shrink-0 rounded-xs bg-hover"
      style={{ width: prominent ? 20 : 14, height: prominent ? 20 : 14 }}
    />
  ) : revealDefault ? (
    <FolderOpenIcon size={prominent ? 20 : 14} />
  ) : (
    <ApplicationIcon
      key={preferred?.icon ?? 'none'}
      source={preferred?.icon ?? null}
      size={prominent ? 20 : 14}
    />
  );

  return (
    <Dropdown
      align="end"
      className="shrink-0 self-center"
      surfaceClassName={MENU_CARD}
      menuClassName="min-w-[144px] max-w-[360px] max-h-[calc(100vh-24px)] overflow-y-auto"
      onOpenChange={handleOpenChange}
      trigger={({ toggle }) => (
        <div
          data-open-target="file"
          data-size={prominent ? 'large' : 'compact'}
          data-open-path={prominent ? undefined : ''}
          data-state={disabled ? 'busy' : 'idle'}
          className={`inline-flex items-stretch box-border overflow-hidden hairline border-border-input rounded-sm ${
            prominent ? 'h-9 rounded-md border-border' : 'h-6'
          }`}
        >
          <button
            type="button"
            className={`${SPLIT_HALF} gap-1 ${
              prominent ? 'gap-1.5 px-3.5 py-1.5 text-body' : 'px-[5px] py-[3px]'
            }`}
            disabled={disabled}
            aria-label={prominent ? undefined : primaryLabel}
            title={primaryLabel}
            data-open-path-open={prominent ? undefined : ''}
            data-open-path-unpreviewable={prominent ? '' : undefined}
            onClick={primary}
          >
            {icon}
            {prominent && (revealDefault ? '显示文件位置' : '打开')}
          </button>
          {hasMenu && (
            <button
              type="button"
              className={`${SPLIT_HALF} pl-[3px] pr-1 py-[3px] text-text-secondary hairline-l border-border-input ${
                prominent ? 'px-2.5 py-1.5' : ''
              }`}
              disabled={disabled}
              aria-haspopup="menu"
              aria-expanded="false"
              aria-label="更多打开方式"
              title="更多打开方式"
              data-open-path-more={prominent ? undefined : ''}
              onClick={toggle}
            >
              <ChevronDownIcon size={prominent ? 14 : 12} />
            </button>
          )}
        </div>
      )}
    >
      {apps.map((app) => (
        <button
          key={app.id}
          type="button"
          role="menuitem"
          className={`${MENU_ITEM} hover:bg-hover text-text-primary`}
          title={app.name}
          onClick={() => run(ipc.openWorkspaceFileWith(root, path, app.id))}
        >
          <span className="shrink-0 w-3.5 h-3.5 inline-flex items-center justify-center">
            <ApplicationIcon key={app.icon ?? 'none'} source={app.icon} />
          </span>
          <span className="flex-1 min-w-0 truncate">
            {app.id === preferred?.id ? `${app.name}（默认）` : app.name}
          </span>
        </button>
      ))}
      {failed && (
        <button type="button" role="menuitem" disabled className={`${MENU_ITEM} opacity-40`}>
          <span className="flex-1 min-w-0 truncate">无法获取应用列表</span>
        </button>
      )}
      <div className="mt-[3px] pt-[3px] flex flex-col hairline-t border-border">
        <button
          type="button"
          role="menuitem"
          className={`${MENU_ITEM} hover:bg-hover text-text-primary`}
          title={footerLabel}
          onClick={() => run(ipc.revealWorkspaceFile(root, path))}
        >
          <span className="shrink-0 w-3.5 h-3.5 inline-flex items-center justify-center">
            <FolderOpenIcon size={14} />
          </span>
          <span className="flex-1 min-w-0 truncate">{footerLabel}</span>
        </button>
      </div>
    </Dropdown>
  );
};
