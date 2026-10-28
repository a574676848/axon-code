import React, { useCallback, useEffect, useState } from 'react';
import { AlertCircleIcon, RefreshIcon } from '../../icons';
import { AxonLogo } from '../../icons';
import type { GithubInfo, SystemInfo, UpdateCheck } from '../../types';
import * as ipc from '../../ipc';
import { Button } from '../ui/controls';
import { Banner, Text } from '../ui/surfaces';

/*
 * 关于页：产品身份、版本事实、发布渠道入口与版本检测。
 *
 * 这里只讲「这是什么产品、现在是什么版本、去哪里拿新版本」，因此不放运行状态，
 * 也不放任何操作项：桌面外壳的运行态与配置目录在「系统设置」里，那是它们的归属。
 *
 * 发布渠道的坐标全部由后端从构建期元数据派生（仓库地址、许可标识都写在工作区
 * Cargo.toml 里），界面上按名字给出入口，不铺一串截断的 URL。
 *
 * 检测结论只有三种：已是最新、发现新版本、暂时无法检测。第三种一律用产品口径的
 * 一句话，具体归因收在「详情」里，供需要排查的人展开，而不是糊在首屏。
 */

/** 发布渠道的一项入口。 */
const ChannelLink: React.FC<{ label: string; url: string; onOpen: (url: string) => void }> = ({
  label,
  url,
  onOpen,
}) => (
  <button
    type="button"
    // 地址是这一项唯一的事实，原生提示给全量文本，界面上就不必铺 URL。
    title={url}
    disabled={url.length === 0}
    onClick={() => onOpen(url)}
    className="text-row text-text-link transition-colors duration-100 hover:text-text-primary hover:underline disabled:text-text-tertiary disabled:cursor-not-allowed"
  >
    {label}
  </button>
);

/** 检测结论的产品口径文案。 */
function updateStatus(update: UpdateCheck | null, checking: boolean): string {
  if (checking) return '正在检测…';
  if (update === null) return '尚未检测';
  if (update.error.length > 0) return '暂时无法检测新版本';
  if (update.update_available) return `发现新版本 ${update.latest}`;
  return '已是最新版本';
}

export const SystemInfoSettings: React.FC = () => {
  const [info, setInfo] = useState<SystemInfo | null>(null);
  const [update, setUpdate] = useState<UpdateCheck | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setInfo(await ipc.systemInfo());
      setError(null);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '读取版本信息失败');
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const check = async () => {
    setChecking(true);
    setError(null);
    try {
      setUpdate(await ipc.checkUpdate());
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '检测更新失败');
    } finally {
      setChecking(false);
    }
  };

  /** 打开外部地址：打不开时如实呈现，不留一个点了没反应的入口。 */
  const openUrl = useCallback((url: string) => {
    void ipc
      .openExternalUrl(url)
      .then(() => setError(null))
      .catch((failure: unknown) =>
        setError(failure instanceof Error ? failure.message : `无法打开 ${url}`),
      );
  }, []);

  const github: GithubInfo | null = info?.github ?? null;

  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-6 pb-16 text-center">
      {/* 产品身份 */}
      <div className="flex flex-col items-center gap-4">
        <AxonLogo size={44} />
        <div className="space-y-1">
          <span className="block text-heading-3 leading-6 font-medium text-text-primary">Axon</span>
          <Text tone="secondary" size="xs" className="block">
            版本 {info?.app_version ?? '未知'}
            {github !== null ? ` · ${github.license}` : ''}
          </Text>
        </div>
      </div>

      {/* 发布渠道：真实坐标，按名字给入口 */}
      <div className="flex flex-wrap items-center justify-center gap-x-4 gap-y-2">
        <ChannelLink
          label="项目主页"
          url={github?.repository_url ?? ''}
          onOpen={openUrl}
        />
        <ChannelLink label="发布说明" url={github?.releases_url ?? ''} onOpen={openUrl} />
        <ChannelLink label="问题反馈" url={github?.issues_url ?? ''} onOpen={openUrl} />
      </div>

      {/* 版本检测排在发布渠道之后：结论来自上面那一组坐标 */}
      <div className="flex flex-col items-center gap-2.5">
        <Button
          variant="primary"
          size="md"
          disabled={checking || info === null}
          onClick={() => void check()}
        >
          <RefreshIcon size={13} />
          {checking ? '检测中' : '检测更新'}
        </Button>
        <Text tone="secondary" size="xs">
          {updateStatus(update, checking)}
        </Text>

        {update !== null && update.update_available && update.download_url.length > 0 && (
          <ChannelLink label="前往下载" url={update.download_url} onOpen={openUrl} />
        )}
      </div>

      {error !== null && (
        <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={error} />
      )}
    </div>
  );
};
