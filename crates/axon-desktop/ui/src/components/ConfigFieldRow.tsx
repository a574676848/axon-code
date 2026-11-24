import React, { useState } from 'react';
import type { ConfigKeySpec } from '../types';
import { Button, Input, NumberInput, Switch } from './ui/controls';
import { Select } from './ui/overlays';
import { Text } from './ui/surfaces';
import { KeyIcon } from '../icons';
import { SecretDialog } from './settings/SecretDialog';
import { configDetail, configLabel } from '../configCopy';

/*
 * 单条配置项：左侧只留一行中文友好名称与一行说明，改动即时生效。
 * 真实配置键与只读标记放进悬浮提示，避免每行堆三行文字干扰阅读。
 *
 * 几何：标签与控件首行对齐（items-start），控件列固定 224px。
 * 「配置密钥」放在标签行右端，不进入控件列，因此所有行的输入框
 * 保持同一宽度与同一右轴，不会因为多一个按钮而整体左移。
 */

/** 这些键的取值必须来自已登记的模型目录，而不是自由文本。 */
/// 向量模型键：候选项必须限定为支持向量检索的模型。
const EMBEDDING_MODEL_KEY = 'memory.embedding.model';

const MODEL_REFERENCE_KEYS = new Set([
  'agent.default_model',
  'agent.fast_model',
  'memory.embedding.model',
]);

/** 以这些后缀结尾的键存放的是密钥环境变量名，需要配套的可视化密钥配置入口。 */
const SECRET_ENV_KEY_SUFFIXES = ['api_key_env', 'api_token_env', 'app_secret_env'];

/** 右侧控件列的固定宽度，保证整列对齐。 */
const CONTROL_WIDTH = 'w-56';

/** 悬浮提示：真实配置键加上中文说明，便于对回配置文件。 */
function buildTooltip(spec: ConfigKeySpec): string {
  const detail = configDetail(spec);
  const parts = [configLabel(spec), spec.key];
  if (detail.length > 0) parts.splice(1, 0, detail);
  if (!spec.editable) parts.push('该配置项只读');
  return parts.join('\n');
}

interface ConfigFieldRowProps {
  /** 权威键规格。 */
  spec: ConfigKeySpec;
  /** 界面上当前显示的值。 */
  value: string;
  /** 已登记模型名称，供模型引用类键使用。 */
  modelNames: string[];
  /** 支持向量检索的模型名称，仅向量模型键使用。 */
  embeddingModelNames?: string[];
  /** 取值变化，立即生效。 */
  onChange: (next: string) => void;
  /** 该行是否正在写入。 */
  busy?: boolean;
}

export const ConfigFieldRow: React.FC<ConfigFieldRowProps> = ({
  spec,
  value,
  modelNames,
  embeddingModelNames = [],
  onChange,
  busy = false,
}) => {
  // 向量模型只能从支持向量检索的模型里选，其余模型引用键列出全部已登记模型。
  const candidateModels = spec.key === EMBEDDING_MODEL_KEY ? embeddingModelNames : modelNames;
  const referenceOptions = Array.from(new Set(candidateModels));

  const label = configLabel(spec);
  const detail = configDetail(spec);
  const [secretOpen, setSecretOpen] = useState(false);
  const secretEnvKey = SECRET_ENV_KEY_SUFFIXES.some((suffix) => spec.key.endsWith(suffix));

  const control = (() => {
    // 布尔项优先判定：启用禁用一类配置即使带 true/false 候选，也必须用开关而不是下拉。
    if (spec.kind === 'bool') {
      return (
        <div className="flex justify-end">
          <Switch
            label={label}
            checked={value === 'true'}
            disabled={!spec.editable || busy}
            onChange={(next) => onChange(next ? 'true' : 'false')}
          />
        </div>
      );
    }
    if (MODEL_REFERENCE_KEYS.has(spec.key)) {
      return (
        <Select
          label={label}
          value={value}
          disabled={!spec.editable || busy}
          options={referenceOptions.map((name) => ({ value: name, label: name }))}
          onChange={onChange}
        />
      );
    }
    if (spec.options.length > 0) {
      return (
        <Select
          label={label}
          value={value}
          disabled={!spec.editable || busy}
          options={spec.options.map((option) => ({ value: option, label: option }))}
          onChange={onChange}
        />
      );
    }
    if (spec.kind === 'unsigned') {
      return (
        <NumberInput
          key={`${value}:${busy}`}
          label={label}
          value={Number(value) || 0}
          min={0}
          disabled={!spec.editable || busy}
          onChange={(next) => onChange(String(next))}
        />
      );
    }
    return (
      <Input
        aria-label={label}
        disabled={!spec.editable || busy}
        defaultValue={value}
        key={`${value}:${busy}`}
        onBlur={(event) => {
          if (event.target.value !== value) onChange(event.target.value);
        }}
        onKeyDown={(event) => {
          if (event.key === 'Enter') {
            event.currentTarget.blur();
          }
        }}
      />
    );
  })();

  return (
    <>
      <div className="flex items-start justify-between gap-6 py-3.5" title={buildTooltip(spec)}>
        <div className="min-w-0 flex-1">
          <div className="flex items-center justify-between gap-3">
            <Text truncate className="min-w-0">
              {label}
            </Text>
            {secretEnvKey && (
              <Button
                variant="ghost"
                size="sm"
                disabled={(value ?? '').trim().length === 0}
                onClick={() => setSecretOpen(true)}
              >
                <KeyIcon size={13} />
                配置密钥
              </Button>
            )}
          </div>
          {detail.length > 0 && (
            <Text tone="secondary" size="xs" className="mt-0.5 block">
              {detail}
            </Text>
          )}
        </div>
        <div className={`shrink-0 ${CONTROL_WIDTH}`}>{control}</div>
      </div>

      {secretEnvKey && (
        <SecretDialog
          open={secretOpen}
          envKey={value}
          onClose={() => setSecretOpen(false)}
          onChanged={() => undefined}
        />
      )}
    </>
  );
};
