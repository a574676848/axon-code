//! 单个语音渠道的编辑弹窗：渠道字段与该渠道名下的模型清单一起改，一次保存。
//!
//! 形态对齐模型管理（卡片清单、弹窗编辑、显式保存），表单与行列表全部复用 `ui`
//! 下的既有原语与 `ModelListEditor` 的行骨架，不另起一套控件。
//!
//! 模型归属由所在渠道决定，不在行内再填一次渠道名：渠道改名时由调用方统一同步，
//! 单条模型无法把归属改到别的渠道，也就不会出现跨渠道的悬空引用。

import React, { useState } from 'react';
import { AlertCircleIcon, PlusIcon, TrashIcon } from '../../icons';
import type { VoiceChannelDto, VoiceModelDto } from '../../types';
import { Button, Input, NumberInput, Switch } from '../ui/controls';
import { Dialog } from '../ui/modal';
import {
  Banner,
  EmptyState,
  FormField,
  Hairline,
  Panel,
  SectionHeading,
  Text,
  ToggleField,
} from '../ui/surfaces';

/** 渠道协议候选：与后端常量同名，避免界面能选、配置校验不认。 */
const PROTOCOLS = ['volcengine-ark-multimodal'];
/** 传输方式候选。 */
const TRANSPORTS = ['websocket'];
/** 模型角色候选。 */
const ROLES = ['asr', 'tts', 'realtime'];
/** 轮次检测候选。 */
const TURNS = ['server_vad', 'smart_turn_v2', 'manual'];

/** 新增模型行的初始值；渠道名由所属渠道在保存时统一写入。 */
const EMPTY_MODEL: VoiceModelDto = {
  name: '',
  channel: '',
  id: '',
  role: 'tts',
  voice: '',
  speechRate: 0,
  turnDetection: 'server_vad',
  priority: 0,
  weight: 1,
};

export interface VoiceChannelDialogProps {
  /** 是否打开。 */
  open: boolean;
  /** 弹窗标题。 */
  title: string;
  /** 打开时的渠道快照；为 null 表示没有可编辑对象。 */
  channel: VoiceChannelDto | null;
  /** 该渠道名下的模型快照。 */
  models: VoiceModelDto[];
  /** 保存进行中。 */
  busy: boolean;
  /** 关闭。 */
  onClose: () => void;
  /** 保存：渠道与该渠道的完整模型清单一次交给调用方落盘。 */
  onSave: (channel: VoiceChannelDto, models: VoiceModelDto[]) => void;
  /** 删除渠道，连带其名下模型；二次确认在本组件内完成，调用方只管清理与落盘。 */
  onDelete: () => void;
}

/**
 * 渠道编辑弹窗。
 *
 * 草稿只在打开这一刻按快照建立，之后不再跟随属性变化重置：调用方用 `key` 控制
 * 重建，保存失败时弹窗不关闭，用户已填的内容也就不会被父组件的重渲染冲掉。
 */
export const VoiceChannelDialog: React.FC<VoiceChannelDialogProps> = ({
  open,
  title,
  channel,
  models,
  busy,
  onClose,
  onSave,
  onDelete,
}) => {
  const [draft, setDraft] = useState<VoiceChannelDto | null>(
    channel === null ? null : { ...channel },
  );
  const [draftModels, setDraftModels] = useState<VoiceModelDto[]>(
    models.map((model) => ({ ...model })),
  );
  const [fieldError, setFieldError] = useState<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);

  if (draft === null) {
    return null;
  }

  const editChannel = (change: Partial<VoiceChannelDto>) => {
    setDraft((prev) => (prev === null ? prev : { ...prev, ...change }));
    setFieldError(null);
  };

  const editModel = (index: number, change: Partial<VoiceModelDto>) => {
    setDraftModels((prev) =>
      prev.map((model, at) => (at === index ? { ...model, ...change } : model)),
    );
    setFieldError(null);
  };

  const submit = () => {
    if (draft.name.trim().length === 0) {
      setFieldError('请填写渠道名称');
      return;
    }
    const unnamed = draftModels.findIndex((model) => model.name.trim().length === 0);
    if (unnamed >= 0) {
      setFieldError(`第 ${unnamed + 1} 条模型还没有名称`);
      return;
    }
    const missingVoice = draftModels.findIndex(
      (model) => model.role !== 'asr' && model.voice.trim().length === 0,
    );
    if (missingVoice >= 0) {
      setFieldError(`第 ${missingVoice + 1} 条模型的角色是 tts 或 realtime，音色必填`);
      return;
    }
    onSave({ ...draft, name: draft.name.trim() }, draftModels);
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={title}
      size="md"
      hint="保存时整段写回；渠道改名会同步它名下模型的归属"
      primary={{
        label: '保存渠道',
        busyLabel: '保存中',
        busy,
        onClick: submit,
      }}
    >
      <div className="grid grid-cols-2 gap-2">
        <FormField label="渠道名称">
          <Input
            value={draft.name}
            placeholder="段内唯一，模型按它归属"
            onChange={(event) => editChannel({ name: event.target.value })}
          />
        </FormField>
        <FormField label="协议">
          <select
            aria-label="渠道协议"
            className="h-8 w-full px-2.5 rounded-sm bg-surface text-row text-text-primary"
            value={draft.protocol}
            onChange={(event) => editChannel({ protocol: event.target.value })}
          >
            {PROTOCOLS.map((protocol) => (
              <option key={protocol} value={protocol}>
                {protocol}
              </option>
            ))}
          </select>
        </FormField>
        <FormField label="API 地址">
          <Input
            value={draft.baseUrl}
            placeholder="https://..."
            onChange={(event) => editChannel({ baseUrl: event.target.value })}
          />
        </FormField>
        <FormField label="WebSocket 地址">
          <Input
            value={draft.websocketUrl ?? ''}
            placeholder="留空则按协议推导"
            onChange={(event) => editChannel({ websocketUrl: event.target.value || null })}
          />
        </FormField>
        <FormField label="密钥环境变量名">
          <Input
            value={draft.apiKeyEnv ?? ''}
            placeholder={draft.apiKeyConfigured ? '已配置（留空保留）' : '例如 ARK_KEY'}
            onChange={(event) => editChannel({ apiKeyEnv: event.target.value || null })}
          />
        </FormField>
        <FormField label="传输方式">
          <select
            aria-label="传输方式"
            className="h-8 w-full px-2.5 rounded-sm bg-surface text-row text-text-primary"
            value={draft.wireTransport}
            onChange={(event) => editChannel({ wireTransport: event.target.value })}
          >
            {TRANSPORTS.map((transport) => (
              <option key={transport} value={transport}>
                {transport}
              </option>
            ))}
          </select>
        </FormField>
        <FormField label="优先级">
          <NumberInput
            label="渠道优先级"
            value={draft.priority}
            step={1}
            onChange={(next) => editChannel({ priority: next })}
          />
        </FormField>
        <FormField label="权重">
          <NumberInput
            label="渠道权重"
            value={draft.weight}
            min={1}
            step={1}
            onChange={(next) => editChannel({ weight: next })}
          />
        </FormField>
      </div>

      <ToggleField
        label="启用该渠道"
        description="停用后，该渠道名下的模型不会进入候选链"
        control={
          <Switch
            label="启用该渠道"
            checked={draft.enabled}
            onChange={(next) => editChannel({ enabled: next })}
          />
        }
      />

      <Hairline />

      <SectionHeading
        title="语音模型"
        description={`该渠道名下 ${draftModels.length} 条模型`}
        action={
          <Button
            variant="outline"
            size="sm"
            onClick={() =>
              setDraftModels((prev) => [...prev, { ...EMPTY_MODEL, channel: draft.name }])
            }
          >
            <PlusIcon size={13} />
            添加模型
          </Button>
        }
      />

      <Panel clip>
        {draftModels.length === 0 ? (
          <EmptyState title="尚未配置模型" description="点「添加模型」为该渠道配一条" />
        ) : (
          <div className="divide-hairline divide-border-hairline">
            {draftModels.map((model, index) => (
              <div key={`voice-model-${index}`} className="px-3 py-3 space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <Text tone="secondary" size="xs" truncate className="min-w-0">
                    {model.name.length > 0
                      ? `模型 ${index + 1} · ${model.name}`
                      : `模型 ${index + 1}`}
                  </Text>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() =>
                      setDraftModels((prev) => prev.filter((_model, at) => at !== index))
                    }
                  >
                    <TrashIcon size={13} className="text-danger" />
                    <span className="text-danger">移除</span>
                  </Button>
                </div>

                <div className="grid grid-cols-2 gap-2">
                  <FormField label="名称">
                    <Input
                      value={model.name}
                      placeholder="段内唯一"
                      onChange={(event) => editModel(index, { name: event.target.value })}
                    />
                  </FormField>
                  <FormField label="上游模型标识">
                    <Input
                      value={model.id}
                      placeholder="上游真实 ID"
                      onChange={(event) => editModel(index, { id: event.target.value })}
                    />
                  </FormField>
                  <FormField label="音色">
                    <Input
                      value={model.voice}
                      placeholder="tts 与 realtime 必填"
                      onChange={(event) => editModel(index, { voice: event.target.value })}
                    />
                  </FormField>
                  <FormField label="语速，负 50 到 100">
                    <NumberInput
                      label="语速"
                      value={model.speechRate}
                      min={-50}
                      step={5}
                      onChange={(next) => editModel(index, { speechRate: next })}
                    />
                  </FormField>
                  <FormField label="角色">
                    <select
                      aria-label={`模型 ${index + 1} 角色`}
                      className="h-8 w-full px-2.5 rounded-sm bg-surface text-row text-text-primary"
                      value={model.role}
                      onChange={(event) => editModel(index, { role: event.target.value })}
                    >
                      {ROLES.map((role) => (
                        <option key={role} value={role}>
                          {role}
                        </option>
                      ))}
                    </select>
                  </FormField>
                  <FormField label="轮次检测">
                    <select
                      aria-label={`模型 ${index + 1} 轮次检测`}
                      className="h-8 w-full px-2.5 rounded-sm bg-surface text-row text-text-primary"
                      value={model.turnDetection}
                      onChange={(event) => editModel(index, { turnDetection: event.target.value })}
                    >
                      {TURNS.map((turn) => (
                        <option key={turn} value={turn}>
                          {turn}
                        </option>
                      ))}
                    </select>
                  </FormField>
                  <FormField label="优先级">
                    <NumberInput
                      label={`模型 ${index + 1} 优先级`}
                      value={model.priority}
                      step={1}
                      onChange={(next) => editModel(index, { priority: next })}
                    />
                  </FormField>
                  <FormField label="权重">
                    <NumberInput
                      label={`模型 ${index + 1} 权重`}
                      value={model.weight}
                      min={1}
                      step={1}
                      onChange={(next) => editModel(index, { weight: next })}
                    />
                  </FormField>
                </div>
              </div>
            ))}
          </div>
        )}
      </Panel>

      {fieldError !== null && (
        <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={fieldError} />
      )}

      <Hairline />
      <div className="flex items-center justify-between gap-3">
        <Text tone="secondary" size="xs" className="min-w-0">
          {confirmingDelete
            ? `删除后不可恢复，该渠道名下的 ${draftModels.length} 条模型会一并删除`
            : '删除渠道会同时删除它名下的模型'}
        </Text>
        <div className="shrink-0 flex items-center gap-1.5">
          {confirmingDelete && (
            <Button
              variant="ghost"
              size="sm"
              disabled={busy}
              onClick={() => setConfirmingDelete(false)}
            >
              取消
            </Button>
          )}
          <Button
            variant="danger"
            size="sm"
            disabled={busy}
            onClick={() => {
              if (!confirmingDelete) {
                setConfirmingDelete(true);
                return;
              }
              onDelete();
            }}
          >
            <TrashIcon size={13} />
            {confirmingDelete ? '确认删除' : '删除渠道'}
          </Button>
        </div>
      </div>
    </Dialog>
  );
};
