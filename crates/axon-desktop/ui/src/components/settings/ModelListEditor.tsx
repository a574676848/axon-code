import React, { useState } from 'react';
import { AlertCircleIcon, PlusIcon, TrashIcon } from '../../icons';
import type { DiscoveredModel, DiscoverModelsResult, ModelInput } from '../../types';
import * as ipc from '../../ipc';
import { Button, Input, NumberInput, Switch } from '../ui/controls';
import { Banner, EmptyState, FormField, Panel, SectionHeading, Text, ToggleField } from '../ui/surfaces';
import { DiscoverModelsModal } from './DiscoverModelsModal';

/*
 * 一个供应商名下的模型清单。
 * 每一行都是一条草稿，整张卡片保存时才随供应商一起原子落盘；
 * 「获取模型」把上游返回的结果作为候选交给用户勾选，绝不自动写配置。
 */

/** 新建模型行的初始值。 */
const EMPTY_MODEL: Omit<ModelInput, 'provider'> = {
  name: '',
  id: '',
  context_window: 128000,
  max_output_tokens: 8192,
  supports_vision: true,
  supports_embedding: false,
  embedding_dimension: 0,
};

interface ModelListEditorProps {
  /** 该供应商名下的模型草稿。 */
  models: ModelInput[];
  /** 供应商名称，用于归属与探测。 */
  providerName: string;
  /** 供应商接口地址。 */
  baseUrl: string;
  /** 供应商协议。 */
  protocol: string;
  /** 密钥环境变量名。 */
  apiKeyEnv?: string;
  /** 是否禁用编辑。 */
  disabled?: boolean;
  /** 草稿变化。 */
  onChange: (models: ModelInput[]) => void;
}

export const ModelListEditor: React.FC<ModelListEditorProps> = ({
  models,
  providerName,
  baseUrl,
  protocol,
  apiKeyEnv,
  disabled = false,
  onChange,
}) => {
  const [fetching, setFetching] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [answer, setAnswer] = useState<DiscoverModelsResult | null>(null);

  const patch = (index: number, next: Partial<ModelInput>) => {
    onChange(models.map((model, at) => (at === index ? { ...model, ...next } : model)));
  };

  const fetchModels = async () => {
    setFetching(true);
    setFailure(null);
    try {
      const result = await ipc.discoverModels(
        providerName.length > 0 ? providerName : undefined,
        baseUrl.length > 0 ? baseUrl : undefined,
        protocol.length > 0 ? protocol : undefined,
        apiKeyEnv !== undefined && apiKeyEnv.length > 0 ? apiKeyEnv : undefined,
      );
      setAnswer(result);
    } catch (error) {
      setFailure(error instanceof Error ? error.message : '获取模型列表失败');
    } finally {
      setFetching(false);
    }
  };

  /** 采用候选：已存在的模型保留用户校正过的参数，不覆盖。 */
  const adopt = (picked: DiscoveredModel[]) => {
    const byId = new Map(models.map((model) => [model.id, model] as const));
    for (const candidate of picked) {
      if (byId.has(candidate.id)) continue;
      byId.set(candidate.id, {
        ...EMPTY_MODEL,
        name: candidate.name ?? candidate.id,
        id: candidate.id,
        provider: providerName,
        context_window: candidate.context_window ?? EMPTY_MODEL.context_window,
        max_output_tokens: candidate.max_output_tokens ?? EMPTY_MODEL.max_output_tokens,
      });
    }
    onChange([...byId.values()]);
    setAnswer(null);
  };

  return (
    <section className="space-y-2" aria-label="模型清单">
      <SectionHeading
        title="模型"
        description={`该供应商名下已配置 ${models.length} 条模型`}
        action={
          <Button
            variant="outline"
            size="sm"
            disabled={disabled || fetching || baseUrl.trim().length === 0}
            title={baseUrl.trim().length === 0 ? '请先填写接口地址' : undefined}
            onClick={() => void fetchModels()}
          >
            {fetching ? '获取中' : '获取模型'}
          </Button>
        }
      />

      {failure !== null && (
        <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={failure} />
      )}

      <Panel clip>
        {models.length === 0 ? (
          <EmptyState
            title="尚未配置模型"
            description="可先获取上游模型列表，或手工添加一条"
          />
        ) : (
          <div className="divide-hairline divide-border-hairline">
            {models.map((model, index) => (
              <div key={`model-${index}`} className="px-3 py-3 space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <Text tone="secondary" size="xs" truncate className="min-w-0">
                    {model.name.length > 0 ? `模型 ${index + 1} · ${model.name}` : `模型 ${index + 1}`}
                  </Text>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={disabled}
                    onClick={() => onChange(models.filter((_entry, at) => at !== index))}
                  >
                    <TrashIcon size={13} className="text-danger" />
                    <span className="text-danger">移除</span>
                  </Button>
                </div>

                <div className="grid grid-cols-2 gap-2">
                  <FormField label="模型名称">
                    <Input
                      value={model.name}
                      disabled={disabled}
                      placeholder="路由键"
                      onChange={(event) => patch(index, { name: event.target.value })}
                    />
                  </FormField>
                  <FormField label="模型 ID">
                    <Input
                      value={model.id}
                      disabled={disabled}
                      placeholder="上游真实 ID"
                      onChange={(event) => patch(index, { id: event.target.value })}
                    />
                  </FormField>
                  <FormField label="上下文窗口">
                    <NumberInput
                      label="上下文窗口"
                      value={model.context_window}
                      min={0}
                      step={1000}
                      disabled={disabled}
                      onChange={(next) => patch(index, { context_window: next })}
                    />
                  </FormField>
                  <FormField label="最大输出 token">
                    <NumberInput
                      label="最大输出 token"
                      value={model.max_output_tokens ?? 0}
                      min={0}
                      step={512}
                      disabled={disabled}
                      onChange={(next) => patch(index, { max_output_tokens: next })}
                    />
                  </FormField>
                </div>

                <ToggleField
                  label="支持视觉输入"
                  control={
                    <Switch
                      label={`模型 ${model.name} 支持视觉`}
                      checked={model.supports_vision}
                      disabled={disabled}
                      onChange={(next) => patch(index, { supports_vision: next })}
                    />
                  }
                />

                <ToggleField
                  label="Embedding 模型"
                  description="仅用于向量检索，不出现在对话模型选择中"
                  control={
                    <Switch
                      label={`模型 ${model.name} 支持 Embedding`}
                      checked={model.supports_embedding}
                      disabled={disabled}
                      onChange={(next) =>
                        patch(index, {
                          supports_embedding: next,
                          // 打开时给一个常用维度，关闭时归零，配置校验只允许二者同时成立。
                          embedding_dimension: next
                            ? model.embedding_dimension > 0
                              ? model.embedding_dimension
                              : 1536
                            : 0,
                        })
                      }
                    />
                  }
                />

                {model.supports_embedding && (
                  <FormField label="Embedding 维度">
                    <NumberInput
                      label="Embedding 维度"
                      value={model.embedding_dimension}
                      min={1}
                      step={256}
                      disabled={disabled}
                      onChange={(next) => patch(index, { embedding_dimension: next })}
                    />
                  </FormField>
                )}
              </div>
            ))}
          </div>
        )}
      </Panel>

      <Button
        variant="outline"
        size="sm"
        disabled={disabled}
        onClick={() => onChange([...models, { ...EMPTY_MODEL, provider: providerName }])}
      >
        <PlusIcon size={13} />
        添加模型
      </Button>

      <DiscoverModelsModal
        open={answer !== null}
        candidates={answer?.models ?? []}
        endpoint={answer?.endpoint ?? ''}
        knownIds={new Set(models.map((model) => model.id))}
        onClose={() => setAnswer(null)}
        onAdopt={adopt}
      />
    </section>
  );
};
