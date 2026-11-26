//! 设置 › 语音：独立 `[voice]` 段的可见可读写页签。
//! 读写走专用 IPC 整段往返；密钥只以变量名呈现，明文由后端保留。

import React, { useCallback, useEffect, useState } from 'react';
import * as ipc from '../../ipc';
import type { ConfigWriteResult, VoiceChannelDto, VoiceConfigDto, VoiceModelDto } from '../../types';
import { Button, Input } from '../ui/controls';
import { EmptyState, SectionHeading, Text, ToggleField } from '../ui/surfaces';
import { Switch } from '../ui/controls';

const LANES = ['cascade', 'realtime', 'auto'];
const ROLES = ['asr', 'tts', 'realtime'];
const TURNS = ['server_vad', 'smart_turn_v2', 'manual'];

const EMPTY_CHANNEL: VoiceChannelDto = {
  name: '',
  enabled: true,
  protocol: 'volcengine-ark-multimodal',
  baseUrl: '',
  apiKeyEnv: null,
  apiKeyConfigured: false,
  wireTransport: 'websocket',
  websocketUrl: null,
  priority: 0,
  weight: 100,
};

const EMPTY_MODEL: VoiceModelDto = {
  name: '',
  channel: '',
  id: '',
  role: 'tts',
  voice: '',
  speechRate: 0,
  turnDetection: 'server_vad',
  instructions: '',
};

/** 语音设置页：整段读取、逐项编辑、一次落盘。 */
export const VoiceSettings: React.FC = () => {
  const [config, setConfig] = useState<VoiceConfigDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setBusy(true);
    try {
      setConfig(await ipc.readVoiceConfig());
      setError(null);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '读取语音配置失败');
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const save = useCallback(async (next: VoiceConfigDto) => {
    setBusy(true);
    try {
      const result: ConfigWriteResult = await ipc.applyVoiceConfig(next);
      if (!result.written) {
        setError(result.message);
      } else {
        setError(null);
      }
      setConfig(await ipc.readVoiceConfig());
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '写入语音配置失败');
    } finally {
      setBusy(false);
    }
  }, []);

  const patch = useCallback((change: (draft: VoiceConfigDto) => void) => {
    setConfig((current) => {
      if (!current) return current;
      const draft: VoiceConfigDto = {
        ...current,
        channels: current.channels.map((item) => ({ ...item })),
        models: current.models.map((item) => ({ ...item })),
      };
      change(draft);
      void save(draft);
      return draft;
    });
  }, [save]);

  if (config === null) {
    return (
      <div className="space-y-3">
        <EmptyState title="读取语音配置" description={busy ? '正在读取…' : '配置暂时不可用'} />
      </div>
    );
  }

  return (
    <div className="space-y-5">
      {error !== null && <Text tone="danger" size="xs">{error}</Text>}

      <section className="space-y-3">
        <ToggleField
          label="启用语音配置"
          description="打开后语音段参与路由；语音交互入口仍由全局开关控制"
          control={
            <Switch
              checked={config.enabled}
              disabled={busy}
              onChange={(next) => patch((draft) => { draft.enabled = next; })}
              label="启用语音配置"
            />
          }
        />
        <div className="space-y-1">
          <Text size="sm">语音车道</Text>
          <select
            aria-label="语音车道"
            className="h-8 px-2.5 rounded-sm bg-surface text-row text-text-primary"
            value={config.lane}
            onChange={(event) => patch((draft) => { draft.lane = event.target.value; })}
          >
            {LANES.map((lane) => <option key={lane} value={lane}>{lane}</option>)}
          </select>
        </div>
      </section>

      <section className="space-y-3">
        <SectionHeading
          title="语音渠道"
          description="渠道即供应商；密钥只写变量名，明文由后端保留"
          action={
            <Button
              variant="outline"
              size="sm"
              disabled={busy}
              onClick={() => patch((draft) => { draft.channels.push({ ...EMPTY_CHANNEL }); })}
            >
              新增渠道
            </Button>
          }
        />
        {config.channels.length === 0 ? (
          <EmptyState title="尚无语音渠道" description="点「新增渠道」开始配置" />
        ) : (
          config.channels.map((channel, index) => (
            <div key={`channel-${index}`} className="space-y-2 hairline-b border-border-hairline pb-3">
              <div className="grid grid-cols-2 gap-2">
                <label className="space-y-1">
                  <Text size="xs" tone="secondary">名称</Text>
                  <Input
                    aria-label={`渠道 ${index + 1} 名称`}
                    value={channel.name}
                    onChange={(event) => patch((draft) => { draft.channels[index].name = event.target.value; })}
                  />
                </label>
                <label className="space-y-1">
                  <Text size="xs" tone="secondary">API 地址</Text>
                  <Input
                    aria-label={`渠道 ${index + 1} API 地址`}
                    value={channel.baseUrl}
                    onChange={(event) => patch((draft) => { draft.channels[index].baseUrl = event.target.value; })}
                  />
                </label>
                <label className="space-y-1">
                  <Text size="xs" tone="secondary">协议</Text>
                  <Input
                    aria-label={`渠道 ${index + 1} 协议`}
                    value={channel.protocol}
                    onChange={(event) => patch((draft) => { draft.channels[index].protocol = event.target.value; })}
                  />
                </label>
                <label className="space-y-1">
                  <Text size="xs" tone="secondary">密钥环境变量名</Text>
                  <Input
                    aria-label={`渠道 ${index + 1} 密钥环境变量名`}
                    value={channel.apiKeyEnv ?? ''}
                    placeholder={channel.apiKeyConfigured ? '已配置（留空保留）' : '例如 VOLC_API_KEY'}
                    onChange={(event) => patch((draft) => { draft.channels[index].apiKeyEnv = event.target.value || null; })}
                  />
                </label>
              </div>
              <Button
                variant="ghost"
                size="sm"
                disabled={busy}
                onClick={() => patch((draft) => { draft.channels.splice(index, 1); })}
              >
                删除渠道
              </Button>
            </div>
          ))
        )}
      </section>

      <section className="space-y-3">
        <SectionHeading
          title="语音模型"
          description="音色按模型配置；tts / realtime 角色必须填写音色"
          action={
            <Button
              variant="outline"
              size="sm"
              disabled={busy}
              onClick={() => patch((draft) => { draft.models.push({ ...EMPTY_MODEL }); })}
            >
              新增模型
            </Button>
          }
        />
        {config.models.length === 0 ? (
          <EmptyState title="尚无语音模型" description="点「新增模型」开始配置" />
        ) : (
          config.models.map((model, index) => (
            <div key={`model-${index}`} className="space-y-2 hairline-b border-border-hairline pb-3">
              <div className="grid grid-cols-2 gap-2">
                <label className="space-y-1">
                  <Text size="xs" tone="secondary">名称</Text>
                  <Input
                    aria-label={`模型 ${index + 1} 名称`}
                    value={model.name}
                    onChange={(event) => patch((draft) => { draft.models[index].name = event.target.value; })}
                  />
                </label>
                <label className="space-y-1">
                  <Text size="xs" tone="secondary">引用渠道</Text>
                  <Input
                    aria-label={`模型 ${index + 1} 引用渠道`}
                    value={model.channel}
                    onChange={(event) => patch((draft) => { draft.models[index].channel = event.target.value; })}
                  />
                </label>
                <label className="space-y-1">
                  <Text size="xs" tone="secondary">上游模型标识</Text>
                  <Input
                    aria-label={`模型 ${index + 1} 上游标识`}
                    value={model.id}
                    onChange={(event) => patch((draft) => { draft.models[index].id = event.target.value; })}
                  />
                </label>
                <label className="space-y-1">
                  <Text size="xs" tone="secondary">音色</Text>
                  <Input
                    aria-label={`模型 ${index + 1} 音色`}
                    value={model.voice}
                    onChange={(event) => patch((draft) => { draft.models[index].voice = event.target.value; })}
                  />
                </label>
              </div>
              {model.role === 'realtime' && (
                <label className="space-y-1">
                  <Text size="xs" tone="secondary">人格提示词（留空用内置语音人格）</Text>
                  <textarea
                    aria-label={`模型 ${index + 1} 人格提示词`}
                    className="w-full min-h-16 px-2.5 py-1.5 rounded-sm bg-surface text-row text-text-primary"
                    value={model.instructions}
                    onChange={(event) => patch((draft) => { draft.models[index].instructions = event.target.value; })}
                  />
                </label>
              )}
              <div className="grid grid-cols-2 gap-2">
                <select
                  aria-label={`模型 ${index + 1} 角色`}
                  className="h-8 px-2.5 rounded-sm bg-surface text-row text-text-primary"
                  value={model.role}
                  onChange={(event) => patch((draft) => { draft.models[index].role = event.target.value; })}
                >
                  {ROLES.map((role) => <option key={role} value={role}>{role}</option>)}
                </select>
                <select
                  aria-label={`模型 ${index + 1} 轮次检测`}
                  className="h-8 px-2.5 rounded-sm bg-surface text-row text-text-primary"
                  value={model.turnDetection}
                  onChange={(event) => patch((draft) => { draft.models[index].turnDetection = event.target.value; })}
                >
                  {TURNS.map((turn) => <option key={turn} value={turn}>{turn}</option>)}
                </select>
              </div>
              <Button
                variant="ghost"
                size="sm"
                disabled={busy}
                onClick={() => patch((draft) => { draft.models.splice(index, 1); })}
              >
                删除模型
              </Button>
            </div>
          ))
        )}
      </section>

      <Button variant="outline" size="sm" disabled={busy} onClick={() => void load()}>
        重新读取
      </Button>
    </div>
  );
};
