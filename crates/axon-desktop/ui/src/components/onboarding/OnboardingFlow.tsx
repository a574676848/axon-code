import React, { useState } from 'react';
import { AlertCircleIcon, CheckIcon, GlobeIcon, RefreshIcon, RetryIcon } from '../../icons';
import type { ConnectionTestResult } from '../../connectionTypes';
import * as ipc from '../../ipc';
import { completeOnboarding, retryProbe, skipOnboarding, snapshot } from '../../connectionState';
import type { SegmentedOption } from '../ui/controls';
import { Button, SegmentedControl } from '../ui/controls';
import { Banner, LoadingState, Panel, Text } from '../ui/surfaces';
import {
  ConnectionProfileDialog,
  ConnectionTestBanner,
  ConnectionTestReadings,
} from '../settings/ConnectionProfileDialog';

/*
 * 首开引导：四步固定顺序，1 选模式，2 本机分支，3 云端分支，4 写入档案并完成。
 *
 * 两条硬边界：
 * 1. 云端分支复用「设置 › 连接」里那份档案表单，不另写一份：测试通过才允许保存，
 *    凭据只在表单里存活，本页只拿结论，不持有任何 Key。
 * 2. 全程可跳过。跳过后进入未连接态，不白屏也不崩：档案可以随后在设置 › 连接 里补，
 *    因此跳过按钮旁边常驻一句话说明跳过的后果，而不是让人先点了才发现没连上。
 *
 * 页面里不出现内部实现名、环境变量名、配置键与文件路径：地址与凭据之外的全部读数
 * 都由后端回执给出。
 */

type ConnectMode = 'local' | 'cloud';

/** 第 1 步选模式，第 2 步与本机后台服务打交道，第 3 步连云电脑，第 4 步落结论。 */
type Stage = 'mode' | 'local' | 'cloud' | 'finish';

const MODE_OPTIONS: readonly SegmentedOption<ConnectMode>[] = [
  { value: 'local', label: '本机安装' },
  { value: 'cloud', label: '连接云电脑' },
];

const STAGE_STEP: Record<Stage, number> = { mode: 1, local: 2, cloud: 3, finish: 4 };

const STAGE_TITLE: Record<Stage, string> = {
  mode: '选择连接方式',
  local: '本机后台服务',
  cloud: '连接云电脑',
  finish: '写入档案并完成',
};

const STAGE_DESCRIPTION: Record<Stage, string> = {
  mode: 'Axon 需要一个后端来干活：装在这一台机器上，或连一台已经跑着后端的云电脑。',
  local: '先探一次本机的后台服务；连不上就从下面的三条出路里挑一条。',
  cloud: '填好后端地址与 API Key，实测通过才会写入档案。',
  finish: '正在记录引导结论，完成后进入工作台。',
};

/** 一条操作回执：成功与失败共用同一个提示位。 */
interface Notice {
  tone: 'success' | 'danger';
  text: string;
}

export const OnboardingFlow: React.FC<{ onDone: () => void }> = ({ onDone }) => {
  const [stage, setStage] = useState<Stage>('mode');
  const [mode, setMode] = useState<ConnectMode>('local');
  const [probing, setProbing] = useState(false);
  const [serviceBusy, setServiceBusy] = useState(false);
  const [guideBusy, setGuideBusy] = useState(false);
  const [finishing, setFinishing] = useState(false);
  /** 进入第 4 步时给出的那一句结论。 */
  const [finishText, setFinishText] = useState('');
  /** 本机探测的结论：只在这一支里用。 */
  const [localError, setLocalError] = useState<string | null>(null);
  /** 落结论与跳过本身没跑通的原因：与「连不上」是两回事，放在页面顶部说。 */
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  /** 云端分支最近一次试连接的结论：本页只保存结论，不保存凭据。 */
  const [cloudResult, setCloudResult] = useState<ConnectionTestResult | null>(null);
  const [cloudDialogOpen, setCloudDialogOpen] = useState(false);

  const messageOf = (caught: unknown, fallback: string): string =>
    caught instanceof Error ? caught.message : fallback;

  /** 第 4 步：记录引导完成，成功即交回外壳接管。 */
  const finish = async (text: string): Promise<void> => {
    setStage('finish');
    setFinishText(text);
    setFinishing(true);
    setActionError(null);
    try {
      await completeOnboarding();
      onDone();
    } catch (caught: unknown) {
      setActionError(messageOf(caught, '记录引导结论失败'));
    } finally {
      setFinishing(false);
    }
  };

  /** 本机分支：探一次；连上即完成，连不上就把原因摊开。 */
  const probeLocal = async (): Promise<void> => {
    setProbing(true);
    setLocalError(null);
    setNotice(null);
    try {
      await retryProbe();
      // 直接从事实源要一份最新快照，不等下一次渲染：这里判断的是刚发生的探测结论。
      const fact = snapshot();
      if (fact.snapshot !== null && fact.snapshot.capsKnown) {
        await finish('本机后台服务已连上。');
        return;
      }
      setLocalError(fact.snapshot?.health.detail ?? fact.error ?? '还没连上本机后台服务');
    } finally {
      setProbing(false);
    }
  };

  const choose = async (): Promise<void> => {
    setLocalError(null);
    setActionError(null);
    setNotice(null);
    if (mode === 'local') {
      setStage('local');
      await probeLocal();
      return;
    }
    setStage('cloud');
  };

  /** 跳到出口：记录跳过后进入未连接态，界面不会因此白屏。 */
  const skip = async (): Promise<void> => {
    setActionError(null);
    try {
      await skipOnboarding();
      onDone();
    } catch (caught: unknown) {
      setActionError(messageOf(caught, '跳过引导失败'));
    }
  };

  /** 本机分支的出路之一：启动后台服务，成功后再探一次。 */
  const startService = async (): Promise<void> => {
    setServiceBusy(true);
    setNotice(null);
    try {
      const result = await ipc.gatewayStartService('start');
      if (!result.success) {
        const output = result.output.trim();
        setNotice({
          tone: 'danger',
          text: output.length > 0 ? `${result.message}：${output}` : result.message,
        });
        return;
      }
      setNotice({ tone: 'success', text: result.message });
      await probeLocal();
    } catch (caught: unknown) {
      setNotice({ tone: 'danger', text: messageOf(caught, '启动后台服务失败') });
    } finally {
      setServiceBusy(false);
    }
  };

  /**
   * 本机分支的出路之二：打开安装说明。
   *
   * 地址取自后端上报的仓库主页，界面里不写死任何外部地址；没有地址就如实说明，
   * 而不是给一个点了没反应的入口。
   */
  const openInstallGuide = async (): Promise<void> => {
    setGuideBusy(true);
    setNotice(null);
    try {
      const info = await ipc.systemInfo();
      const url = info.github.repository_url;
      if (url.length === 0) {
        setNotice({ tone: 'danger', text: '当前版本没有提供安装说明的地址。' });
        return;
      }
      await ipc.openExternalUrl(url);
    } catch (caught: unknown) {
      setNotice({ tone: 'danger', text: messageOf(caught, '打开安装说明失败') });
    } finally {
      setGuideBusy(false);
    }
  };

  const localFailure = (
    <div className="space-y-2">
      <Banner
        tone="danger"
        icon={<AlertCircleIcon size={13} />}
        title={localError ?? '还没连上本机后台服务'}
      />
      <Text tone="secondary" size="xs" className="block">
        三条出路：启动本机后台服务、查看安装说明，或改连一台云电脑。
      </Text>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="outline"
          size="sm"
          disabled={serviceBusy}
          onClick={() => void startService()}
        >
          <RetryIcon size={13} />
          {serviceBusy ? '正在启动' : '启动后台服务'}
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={guideBusy}
          title="在项目主页查看安装与部署说明"
          onClick={() => void openInstallGuide()}
        >
          <GlobeIcon size={13} />
          {guideBusy ? '正在打开' : '打开安装说明'}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            setMode('cloud');
            setLocalError(null);
            setNotice(null);
            setStage('cloud');
          }}
        >
          改选云端
        </Button>
      </div>
    </div>
  );

  return (
    <div className="flex h-full min-h-0 w-full items-center justify-center overflow-y-auto scroll-visible px-6 py-8">
      <div className="w-full max-w-lg space-y-3">
        <div className="space-y-1">
          <Text tone="tertiary" size="micro" className="block">
            {`第 ${STAGE_STEP[stage]} 步，共 4 步`}
          </Text>
          <Text size="base" strong className="block">
            {STAGE_TITLE[stage]}
          </Text>
          <Text tone="secondary" size="xs" className="block">
            {STAGE_DESCRIPTION[stage]}
          </Text>
        </div>

        {actionError !== null && (
          <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={actionError} />
        )}

        <Panel clip>
          <div className="px-3.5 py-3 space-y-3">
            {stage === 'mode' && (
              <div className="space-y-2">
                <SegmentedControl
                  label="连接方式"
                  value={mode}
                  options={MODE_OPTIONS}
                  onChange={setMode}
                />
                <Text tone="secondary" size="xs" className="block">
                  {mode === 'local'
                    ? '本机安装：Axon 与后端在同一台机器上，凭据读本机已有的配置。'
                    : '连接云电脑：后端在远端，填地址与 API Key 后实测连通性。'}
                </Text>
              </div>
            )}

            {stage === 'local' && (probing ? <LoadingState label="正在探测本机后台服务" /> : localFailure)}

            {stage === 'cloud' && (
              <div className="space-y-2">
                <Text tone="secondary" size="xs" className="block">
                  连不上、凭据不匹配、证书有问题都不会写入档案；只有实测通过才会保存。
                </Text>
                {cloudResult !== null && (
                  <div className="space-y-2">
                    <ConnectionTestBanner result={cloudResult} />
                    {cloudResult.conclusion === 'ok' && cloudResult.wsConclusion === 'ok' && (
                      <ConnectionTestReadings result={cloudResult} />
                    )}
                    {cloudResult.plaintextPrivate && (
                      <Banner
                        tone="danger"
                        icon={<AlertCircleIcon size={13} />}
                        title="这个地址使用明文传输，凭据将以明文过网。"
                      />
                    )}
                  </div>
                )}
              </div>
            )}

            {stage === 'finish' &&
              (finishing ? (
                <LoadingState label="正在写入档案" />
              ) : (
                <div className="space-y-2">
                  <Text tone="secondary" size="xs" className="block">
                    {finishText.length > 0 ? finishText : '引导结论已记录。'}
                  </Text>
                  {actionError !== null && (
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => void finish(finishText)}
                    >
                      重试
                    </Button>
                  )}
                </div>
              ))}

            {notice !== null && (
              <Banner
                tone={notice.tone}
                icon={
                  notice.tone === 'success' ? (
                    <CheckIcon size={13} />
                  ) : (
                    <AlertCircleIcon size={13} />
                  )
                }
                title={notice.text}
              />
            )}
          </div>

          {stage !== 'finish' && (
            <div className="px-3.5 py-3 hairline-t border-border-hairline flex flex-wrap items-center gap-2">
              {stage === 'mode' ? (
                <Button variant="primary" size="md" onClick={() => void choose()}>
                  继续
                </Button>
              ) : (
                <>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => {
                      setStage('mode');
                      setLocalError(null);
                      setNotice(null);
                    }}
                  >
                    返回
                  </Button>
                  {stage === 'local' && (
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={probing}
                      onClick={() => void probeLocal()}
                    >
                      <RefreshIcon size={13} />
                      {probing ? '正在探测' : '重新探测'}
                    </Button>
                  )}
                  {stage === 'cloud' && (
                    <Button
                      variant="primary"
                      size="md"
                      onClick={() => {
                        setActionError(null);
                        setCloudDialogOpen(true);
                      }}
                    >
                      {/* 实测通过只是结论，档案要保存才算写入：按钮名字随后改成这一步该做的事。 */}
                      {cloudResult !== null &&
                      cloudResult.conclusion === 'ok' &&
                      cloudResult.wsConclusion === 'ok'
                        ? '返回表单保存'
                        : '填写地址与密钥'}
                    </Button>
                  )}
                </>
              )}
            </div>
          )}
        </Panel>

        <div className="flex items-start justify-between gap-3">
          <Text tone="tertiary" size="xs" className="min-w-0">
            跳过后面板处于未连接状态，可随后在设置里的「连接」分区配置后端。
          </Text>
          <Button
            variant="ghost"
            size="sm"
            disabled={finishing}
            className="shrink-0"
            onClick={() => void skip()}
          >
            跳过
          </Button>
        </div>

        <ConnectionProfileDialog
          open={cloudDialogOpen}
          title="连接云电脑"
          submitLabel="保存档案"
          onClose={() => setCloudDialogOpen(false)}
          onTested={(result) => setCloudResult(result)}
          onSaved={() => {
            setCloudDialogOpen(false);
            void finish('云端档案已写好，可在设置里的「连接」分区切换并重启生效。');
          }}
        />
      </div>
    </div>
  );
};
