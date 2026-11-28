//! 设置 › 语音：先列渠道，点「编辑」进弹窗改渠道字段与它名下的模型。
//!
//! 交互对齐模型管理：卡片清单负责选对象，弹窗负责编辑，保存时整段落盘。
//! 读写走专用 IPC 整段往返；密钥只以变量名呈现，明文由后端保留。
//!
//! 模型归属由所在渠道决定：改名时统一同步引用，删渠道时连带删除它名下的模型，
//! 这样 `[voice].models[].channel` 永远不会指向不存在的渠道。

import React, { useCallback, useEffect, useState } from 'react';
import { PlusIcon, TrashIcon } from '../../icons';
import * as ipc from '../../ipc';
import type { ConfigWriteResult, VoiceChannelDto, VoiceConfigDto, VoiceModelDto } from '../../types';
import { CardList, EntityCard } from '../ui/cards';
import { Button } from '../ui/controls';
import { Chip, EmptyState, Panel, SectionHeading, Text } from '../ui/surfaces';
import { VoiceChannelDialog } from './VoiceChannelDialog';
import { VoiceInputSettings } from './VoiceInputSettings';

/** 新增渠道的初始值；名称在追加时补成段内唯一。 */
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

/** 生成段内唯一的渠道默认名，省去用户新增后还要先解决重名。 */
const nextChannelName = (channels: VoiceChannelDto[]): string => {
  for (let index = channels.length + 1; ; index += 1) {
    const candidate = `渠道 ${index}`;
    if (!channels.some((channel) => channel.name === candidate)) {
      return candidate;
    }
  }
};

/** 语音设置页：渠道清单、弹窗编辑、整段落盘。 */
export const VoiceSettings: React.FC = () => {
  const [config, setConfig] = useState<VoiceConfigDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** 正在编辑的渠道下标；null 表示弹窗关闭。 */
  const [editing, setEditing] = useState<number | null>(null);

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

  /** 整段写盘：写成功才回读，失败原因进顶部错误条。 */
  const commit = useCallback(async (next: VoiceConfigDto): Promise<boolean> => {
    setBusy(true);
    try {
      const result: ConfigWriteResult = await ipc.applyVoiceConfig(next);
      if (!result.written) {
        setError(result.message);
        return false;
      }
      setError(null);
      setConfig(await ipc.readVoiceConfig());
      return true;
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '写入语音配置失败');
      return false;
    } finally {
      setBusy(false);
    }
  }, []);

  /** 新增渠道：先落盘再打开编辑，弹窗里拿到的就是已存在的那一条。 */
  const addChannel = async () => {
    if (config === null) return;
    const channel: VoiceChannelDto = { ...EMPTY_CHANNEL, name: nextChannelName(config.channels) };
    const next: VoiceConfigDto = { ...config, channels: [...config.channels, channel] };
    if (await commit(next)) {
      setEditing(next.channels.length - 1);
    }
  };

  /**
   * 保存弹窗：替换渠道，并按模型名就地对齐归属。
   *
   * 就地替换而不是整段重建，是为了保住声明顺序：`ranked_models` 在优先级与权重
   * 都相同时按声明顺序兜底，顺序抖动会让选路结果无端变化。
   */
  const saveChannel = async (index: number, channel: VoiceChannelDto, models: VoiceModelDto[]) => {
    if (config === null) return;
    const previousName = config.channels[index].name;
    const channels = config.channels.map((item, at) => (at === index ? channel : item));
    const carried = config.models.map((model) =>
      model.channel === previousName ? { ...model, channel: channel.name } : { ...model },
    );
    const incoming = models.map((model) => ({ ...model, channel: channel.name }));
    const reused = new Set<number>();
    const merged: VoiceModelDto[] = [];
    for (const existing of carried) {
      if (existing.channel !== channel.name) {
        merged.push(existing);
        continue;
      }
      const at = incoming.findIndex(
        (model, position) => !reused.has(position) && model.name === existing.name,
      );
      if (at >= 0) {
        reused.add(at);
        merged.push(incoming[at]);
      }
    }
    incoming.forEach((model, position) => {
      if (!reused.has(position)) merged.push(model);
    });
    if (await commit({ channels, models: merged })) {
      setEditing(null);
    }
  };

  /** 删除渠道：连带它名下的模型，避免留下悬空引用。 */
  const deleteChannel = async (index: number) => {
    if (config === null) return;
    const name = config.channels[index].name;
    const next: VoiceConfigDto = {
      channels: config.channels.filter((_item, at) => at !== index),
      models: config.models.filter((model) => model.channel !== name),
    };
    if (await commit(next)) {
      setEditing(null);
    }
  };

  /** 渠道缺失的模型：只有历史脏配置才会出现，留一个能清理的入口。 */
  const removeOrphan = async (name: string, channelName: string) => {
    if (config === null) return;
    await commit({
      ...config,
      models: config.models.filter(
        (model) => !(model.name === name && model.channel === channelName),
      ),
    });
  };

  if (config === null) {
    return (
      <div className="space-y-3">
        <EmptyState title="读取语音配置" description={busy ? '正在读取…' : '配置暂时不可用'} />
      </div>
    );
  }

  const orphans = config.models.filter(
    (model) => !config.channels.some((channel) => channel.name === model.channel),
  );
  const edited = editing === null ? null : config.channels[editing];

  return (
    <div className="space-y-5">
      {error !== null && (
        <Text tone="danger" size="xs" className="block">
          {error}
        </Text>
      )}

      <VoiceInputSettings />

      <section className="space-y-3">
        <SectionHeading
          title="语音渠道"
          description="渠道即供应商；先配渠道，再在渠道里配它名下的模型"
          action={
            <Button variant="outline" size="sm" disabled={busy} onClick={() => void addChannel()}>
              <PlusIcon size={13} />
              新增渠道
            </Button>
          }
        />
        <CardList
          emptyTitle="尚无语音渠道"
          emptyDescription="点右上角「新增渠道」开始配置"
        >
          {config.channels.map((channel, index) => {
            const owned = config.models.filter((model) => model.channel === channel.name);
            return (
              <EntityCard
                key={`channel-${index}`}
                title={channel.name.length > 0 ? channel.name : `渠道 ${index + 1}`}
                badges={
                  <span className="flex items-center gap-1.5">
                    <Chip>{channel.protocol.length > 0 ? channel.protocol : '未设协议'}</Chip>
                    {!channel.enabled && <Chip tone="warning">已停用</Chip>}
                  </span>
                }
                locator={channel.baseUrl}
                metaNode={
                  <span className="flex items-center gap-1.5 min-w-0">
                    <Text tone="secondary" size="micro" truncate className="min-w-0">
                      {owned.length} 条模型
                    </Text>
                    <Text tone="tertiary" size="micro" className="shrink-0 tabular-nums">
                      优先级 {channel.priority} · 权重 {channel.weight}
                    </Text>
                    <Text tone="tertiary" size="micro" className="shrink-0">
                      {channel.apiKeyEnv === null
                        ? '未设密钥'
                        : channel.apiKeyConfigured
                          ? '密钥已配'
                          : '密钥待配'}
                    </Text>
                  </span>
                }
                onOpen={() => setEditing(index)}
                onInspect={() => setEditing(index)}
                inspectLabel="编辑"
              />
            );
          })}
        </CardList>
      </section>

      {orphans.length > 0 && (
        <section className="space-y-3">
          <SectionHeading
            title="渠道缺失的模型"
            description="这些模型引用的渠道已不存在；补齐渠道或移除它们后再保存"
          />
          <Panel clip>
            <div className="divide-hairline divide-border-hairline">
              {orphans.map((model, index) => (
                <div
                  key={`orphan-${index}`}
                  className="px-3 py-2 flex items-center justify-between gap-2"
                >
                  <Text size="xs" truncate className="min-w-0">
                    {model.name.length > 0 ? model.name : '未命名模型'} · 引用{' '}
                    {model.channel.length > 0 ? model.channel : '空'}
                  </Text>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={busy}
                    onClick={() => void removeOrphan(model.name, model.channel)}
                  >
                    <TrashIcon size={13} className="text-danger" />
                    <span className="text-danger">移除</span>
                  </Button>
                </div>
              ))}
            </div>
          </Panel>
        </section>
      )}

      <Button variant="outline" size="sm" disabled={busy} onClick={() => void load()}>
        重新读取
      </Button>

      {edited !== null && edited !== undefined && (
        <VoiceChannelDialog
          key={`voice-channel-${editing}`}
          open
          title={`渠道配置 · ${edited.name.length > 0 ? edited.name : `渠道 ${(editing ?? 0) + 1}`}`}
          channel={edited}
          models={config.models.filter((model) => model.channel === edited.name)}
          busy={busy}
          onClose={() => setEditing(null)}
          onSave={(channel, models) => void saveChannel(editing ?? 0, channel, models)}
          onDelete={() => void deleteChannel(editing ?? 0)}
        />
      )}
    </div>
  );
};
