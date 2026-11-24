import React, { useEffect, useState } from 'react';
import {
  AlertCircleIcon,
  ApiIcon,
  CheckIcon,
  CloseIcon,
  EditIcon,
  KeyIcon,
  PlusIcon,
  TrashIcon,
} from '../../icons';
import type { ModelInput, ModelItem, ProviderInput, ProviderItem } from '../../types';
import * as ipc from '../../ipc';
import { Button, Input, Switch } from '../ui/controls';
import { Dialog, Select } from '../ui/overlays';
import {
  Banner,
  Chip,
  EmptyState,
  FormField,
  ListRow,
  Panel,
  Text,
  ToggleField,
} from '../ui/surfaces';
import { ModelListEditor } from './ModelListEditor';
import { SecretDialog } from './SecretDialog';
import { SettingsHeaderAction } from './SettingsHeaderSlot';

/*
 * 一张供应商卡片等于一个供应商档案：供应商自身的连接参数加上它名下的模型。
 * 卡片头部常态只显示身份与状态，点「编辑」后在同一张卡片内就地展开编辑区，
 * 保存时整卡一次提交，因此供应商与其模型永远同进同退。
 *
 * 行内只留一个 ghost 入口；移除是破坏性操作，收进展开后的表单底部，
 * 不再让每一行都挂两枚带描边的按钮。
 */

/** 协议候选项由后端透出的权威清单生成，前端不再维护自己的子集。 */
function protocolOptions(protocols: string[]): { value: string; label: string }[] {
  const list = protocols.length > 0 ? protocols : ['openai'];
  return list.map((value) => ({ value, label: value }));
}

/** 链路候选项同样来自后端权威清单。 */
function transportOptions(transports: string[]): { value: string; label: string }[] {
  const list = transports.length > 0 ? transports : ['http'];
  return list.map((value) => ({ value, label: value }));
}

/** 新建供应商的初始值。 */
export const EMPTY_PROVIDER: ProviderInput = {
  name: '',
  enabled: true,
  protocol: 'openai',
  base_url: '',
  api_key_env: '',
  wire_transport: 'http',
  priority: 10,
  weight: 1,
};

/** 把已登记供应商转成可编辑草稿。 */
function toProviderInput(provider: ProviderItem): ProviderInput {
  return {
    name: provider.name,
    enabled: provider.enabled,
    protocol: provider.protocol,
    base_url: provider.base_url,
    api_key_env: provider.api_key_env ?? '',
    wire_transport: provider.wire_transport,
    priority: provider.priority,
    weight: provider.weight,
  };
}

/** 把已登记模型转成可编辑草稿。 */
function toModelInput(model: ModelItem, provider: string): ModelInput {
  return {
    name: model.name,
    id: model.id,
    provider,
    context_window: model.context_window,
    max_output_tokens: model.max_output_tokens,
    supports_vision: model.supports_vision,
    supports_embedding: model.supports_embedding,
    embedding_dimension: model.supports_embedding ? model.embedding_dimension : 0,
    requires_reasoning_content_on_assistant_messages:
      model.requires_reasoning_content_on_assistant_messages,
  };
}

interface ProviderProfileCardProps {
  /** 已登记的供应商；为空表示新建档案。 */
  provider: ProviderItem | null;
  /** 该供应商名下的模型。 */
  models: ModelItem[];
  /** 是否处于展开编辑态。 */
  open: boolean;
  /** 供应商协议候选值。 */
  protocols: string[];
  /** 供应商链路候选值。 */
  wireTransports: string[];
  /** 切换展开状态。 */
  onToggle: () => void;
  /** 任一写入成功后的刷新回调。 */
  onChanged: () => void;
  /** 关闭新建卡片。 */
  onCancelCreate: () => void;
}

export const ProviderProfileCard: React.FC<ProviderProfileCardProps> = ({
  provider,
  models,
  open,
  protocols,
  wireTransports,
  onToggle,
  onChanged,
  onCancelCreate,
}) => {
  const [draft, setDraft] = useState<ProviderInput>(EMPTY_PROVIDER);
  const [draftModels, setDraftModels] = useState<ModelInput[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [secretOpen, setSecretOpen] = useState(false);

  /*
   * 只在「展开的是哪个档案」变化时重新装载草稿。
   *
   * 不能依赖 `provider` 与 `models` 这两个引用：父组件每次渲染都会新建它们
   * （`modelsOf()` 用 filter、新建档案直接给 `[]`），那样这个 effect 会在每次
   * 渲染后重跑，把用户刚输入的草稿立刻覆盖掉，看起来就是「模型填不进去」。
   */
  const identity = provider?.name ?? null;
  const latest = React.useRef({ provider, models });
  latest.current = { provider, models };

  useEffect(() => {
    if (!open) return;
    setError(null);
    setBusy(false);
    const { provider: current, models: currentModels } = latest.current;
    if (current === null) {
      setDraft(EMPTY_PROVIDER);
      setDraftModels([]);
    } else {
      setDraft(toProviderInput(current));
      setDraftModels(currentModels.map((model) => toModelInput(model, current.name)));
    }
  }, [open, identity]);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await ipc.saveProviderProfile(
        draft,
        draftModels.map((model) => ({ ...model, provider: draft.name.trim() })),
        provider?.name,
      );
      onChanged();
      if (provider === null) onCancelCreate();
      else onToggle();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '保存失败');
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (provider === null) return;
    setBusy(true);
    setError(null);
    try {
      await ipc.removeProviderProfile(provider.name);
      setConfirmRemove(false);
      onChanged();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '移除失败');
      setConfirmRemove(false);
    } finally {
      setBusy(false);
    }
  };

  const isCreate = provider === null;
  const name = isCreate ? '新建供应商' : provider.name;

  return (
    <Panel clip>
      {/* 头部：身份与状态在左，唯一入口在右 */}
      <ListRow className="py-2.5">
        <ApiIcon size={15} className="shrink-0 text-text-secondary" />
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 min-w-0">
            <Text strong truncate className="min-w-0">
              {name}
            </Text>
            {!isCreate && <Chip>{provider.protocol}</Chip>}
          </div>
          <Text tone="secondary" size="xs" mono truncate className="block mt-0.5">
            {isCreate
              ? '填写接口地址后即可获取可用模型列表'
              : provider.enabled
                ? provider.base_url
                : `${provider.base_url} · 已停用`}
          </Text>
        </div>

        <div className="shrink-0 flex items-center">
          {isCreate ? (
            <Button variant="ghost" size="sm" onClick={onCancelCreate}>
              <CloseIcon size={13} />
              取消
            </Button>
          ) : (
            <Button variant="ghost" size="sm" onClick={onToggle}>
              <EditIcon size={13} />
              {open ? '收起' : '编辑'}
            </Button>
          )}
        </div>
      </ListRow>

      {/* 展开后的编辑区 */}
      {open && (
        <div className="px-3 py-3 space-y-4 hairline-t border-border-hairline bg-surface">
          {error !== null && (
            <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={error} />
          )}

          <div className="grid grid-cols-2 gap-3">
            <FormField label="供应商标识">
              <Input
                value={draft.name}
                placeholder="唯一名称"
                onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              />
            </FormField>
            <FormField label="协议">
              <Select
                label="协议"
                value={draft.protocol}
                options={protocolOptions(protocols)}
                onChange={(value) => setDraft({ ...draft, protocol: value })}
              />
            </FormField>
            <FormField label="接口地址" className="col-span-2">
              <Input
                value={draft.base_url}
                placeholder="https://example.com/v1"
                onChange={(event) => setDraft({ ...draft, base_url: event.target.value })}
              />
            </FormField>
            <FormField label="密钥环境变量名">
              <div className="flex items-center gap-2">
                <Input
                  value={draft.api_key_env ?? ''}
                  placeholder="例如 PROVIDER_API_KEY"
                  onChange={(event) => setDraft({ ...draft, api_key_env: event.target.value })}
                  className="flex-1"
                />
                <Button
                  variant="outline"
                  size="sm"
                  disabled={(draft.api_key_env ?? '').trim().length === 0}
                  onClick={() => setSecretOpen(true)}
                >
                  <KeyIcon size={13} />
                  配置密钥
                </Button>
              </div>
            </FormField>
            <FormField label="传输链路">
              <Select
                label="传输链路"
                value={draft.wire_transport}
                options={transportOptions(wireTransports)}
                onChange={(value) => setDraft({ ...draft, wire_transport: value })}
              />
            </FormField>
            <FormField label="调度优先级">
              <Input
                inputMode="numeric"
                value={String(draft.priority)}
                onChange={(event) =>
                  setDraft({ ...draft, priority: Number(event.target.value) || 0 })
                }
              />
            </FormField>
            <FormField label="调度权重">
              <Input
                inputMode="numeric"
                value={String(draft.weight)}
                onChange={(event) => setDraft({ ...draft, weight: Number(event.target.value) || 0 })}
              />
            </FormField>
            <div className="col-span-2">
              <ToggleField
                label="参与运行时模型路由"
                description="关闭后该供应商仍保留在配置里，但不参与调度"
                control={
                  <Switch
                    label="启用供应商"
                    checked={draft.enabled}
                    onChange={(next) => setDraft({ ...draft, enabled: next })}
                  />
                }
              />
            </div>
          </div>

          <ModelListEditor
            models={draftModels}
            providerName={draft.name}
            baseUrl={draft.base_url}
            protocol={draft.protocol}
            apiKeyEnv={draft.api_key_env}
            disabled={busy}
            onChange={setDraftModels}
          />

          <div className="flex items-center gap-2">
            {!isCreate && (
              <Button variant="ghost" size="sm" disabled={busy} onClick={() => setConfirmRemove(true)}>
                <TrashIcon size={13} className="text-danger" />
                <span className="text-danger">移除供应商</span>
              </Button>
            )}
            <div className="ml-auto flex items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                disabled={busy}
                onClick={() => (isCreate ? onCancelCreate() : onToggle())}
              >
                <CloseIcon size={13} />
                取消
              </Button>
              <Button
                variant="primary"
                size="sm"
                disabled={busy}
                onClick={() => void save()}
              >
                <CheckIcon size={13} />
                {busy ? '保存中' : '保存'}
              </Button>
            </div>
          </div>
        </div>
      )}

      <Dialog
        open={confirmRemove}
        onClose={() => setConfirmRemove(false)}
        title={`移除供应商 ${name}`}
        size="sm"
        primary={{
          label: '确认移除',
          icon: <TrashIcon size={13} />,
          busy,
          onClick: () => void remove(),
        }}
        secondary={{
          label: '取消',
          icon: <CloseIcon size={13} />,
          disabled: busy,
          onClick: () => setConfirmRemove(false),
        }}
      >
        <Text size="row" className="block leading-relaxed">
          移除后该供应商及其名下模型会一并从配置中删除，此操作不可撤销。
        </Text>
      </Dialog>

      <SecretDialog
        open={secretOpen}
        envKey={draft.api_key_env ?? ''}
        onClose={() => setSecretOpen(false)}
        onChanged={onChanged}
      />
    </Panel>
  );
};

interface ModelsSettingsProps {
  /** 已登记的供应商列表。 */
  providers: ProviderItem[];
  /** 已登记的全部模型。 */
  models: ModelItem[];
  /** 供应商协议候选值。 */
  protocols: string[];
  /** 供应商链路候选值。 */
  wireTransports: string[];
  /** 写入成功后的刷新回调。 */
  onChanged: () => void;
}

export const ModelsSettings: React.FC<ModelsSettingsProps> = ({
  providers,
  models,
  protocols,
  wireTransports,
  onChanged,
}) => {
  const [openName, setOpenName] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const modelsOf = (providerName: string) =>
    models.filter((model) => model.provider === providerName);

  return (
    <div className="space-y-2">
      <SettingsHeaderAction>
        <Button
          variant="outline"
          size="sm"
          disabled={creating}
          onClick={() => {
            setOpenName(null);
            setCreating(true);
          }}
        >
          <PlusIcon size={13} />
          添加供应商
        </Button>
      </SettingsHeaderAction>

      {creating && (
        <ProviderProfileCard
          provider={null}
          models={[]}
          open
          protocols={protocols}
          wireTransports={wireTransports}
          onToggle={() => undefined}
          onChanged={onChanged}
          onCancelCreate={() => setCreating(false)}
        />
      )}

      {providers.length === 0 && !creating ? (
        <EmptyState
          title="尚未配置供应商"
          description="添加一个供应商后即可填写接口地址并获取可用模型"
        />
      ) : (
        providers.map((provider) => (
          <ProviderProfileCard
            key={provider.name}
            provider={provider}
            models={modelsOf(provider.name)}
            open={openName === provider.name}
            protocols={protocols}
            wireTransports={wireTransports}
            onToggle={() =>
              setOpenName((current) => (current === provider.name ? null : provider.name))
            }
            onChanged={onChanged}
            onCancelCreate={() => undefined}
          />
        ))
      )}
    </div>
  );
};
