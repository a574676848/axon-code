import React, { useEffect, useState } from 'react';
import { AlertCircleIcon } from '../../icons';
import { Input, SegmentedControl } from '../ui/controls';
import { Dialog } from '../ui/overlays';
import { Banner, FormField, Text } from '../ui/surfaces';
import type { PriceRule, PriceRuleInput, PricingStyle } from '../../types';
import {
  formatMultiplier,
  formatPricePerMillion,
  normalizePricingStyle,
  parseMultiplier,
  parsePriceInput,
} from './pricingFormat';

/*
 * 价格规则编辑器。
 *
 * 表单里一律按「美元 / 百万 Token」读写：单价在数据模型里是微美元，但没有人用
 * 微美元思考价格。转换只在提交与回填两处发生，并且写成纯函数（pricingFormat），
 * 因此这里不做第二套换算。
 *
 * 「编辑」保存时是先建后删：先建成一条新规则，成功后再删旧规则。中途失败时旧规则
 * 原样保留，不会出现「这个模型突然没有价格」的窗口。
 */

/** 表单里的一行文本输入。 */
const TextField: React.FC<{
  label: string;
  value: string;
  placeholder?: string;
  hint?: string;
  onChange: (next: string) => void;
}> = ({ label, value, placeholder, hint, onChange }) => (
  <FormField label={label}>
    <Input value={value} placeholder={placeholder} onChange={(e) => onChange(e.target.value)} />
    {hint !== undefined && (
      <Text tone="secondary" size="xs" className="block pt-1">
        {hint}
      </Text>
    )}
  </FormField>
);

export interface PricingRuleDialogProps {
  open: boolean;
  onClose: () => void;
  /** 编辑时的原规则；新增时为 undefined。 */
  rule?: PriceRule;
  /** 提交：返回 true 表示已落库并应关闭弹窗。 */
  onSubmit: (input: PriceRuleInput) => Promise<boolean>;
}

export const PricingRuleDialog: React.FC<PricingRuleDialogProps> = ({
  open,
  onClose,
  rule,
  onSubmit,
}) => {
  const [pattern, setPattern] = useState('');
  const [style, setStyle] = useState<PricingStyle>('openai');
  const [input, setInput] = useState('0');
  const [output, setOutput] = useState('0');
  const [cacheRead, setCacheRead] = useState('0');
  const [cacheWrite, setCacheWrite] = useState('0');
  const [multiplier, setMultiplier] = useState('1.0');
  const [priority, setPriority] = useState('0');
  const [busy, setBusy] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);

  const editing = rule !== undefined;

  /*
   * 打开时按目标规则回填表单。
   *
   * 依赖是「是否打开」与「编辑的是哪一条」：切换目标时重新填表，
   * 而不是沿用上一次的输入。
   */
  useEffect(() => {
    if (!open) return;
    setPattern(rule?.modelPattern ?? '');
    setStyle(normalizePricingStyle(rule?.pricingStyle ?? 'openai'));
    setInput(rule === undefined ? '0' : formatPricePerMillion(rule.inputPricePerMMicros));
    setOutput(rule === undefined ? '0' : formatPricePerMillion(rule.outputPricePerMMicros));
    setCacheRead(rule === undefined ? '0' : formatPricePerMillion(rule.cacheReadPricePerMMicros));
    setCacheWrite(rule === undefined ? '0' : formatPricePerMillion(rule.cacheWritePricePerMMicros));
    setMultiplier(rule === undefined ? '1.0' : formatMultiplier(rule.multiplierBasisPoints));
    setPriority(String(rule?.priority ?? 0));
    setLocalError(null);
  }, [open, rule]);

  const submit = async () => {
    const trimmed = pattern.trim();
    if (trimmed.length === 0) {
      setLocalError('模型匹配式不能为空，例如 deepseek-* 或 gpt-5');
      return;
    }
    const prices = {
      input: parsePriceInput(input),
      output: parsePriceInput(output),
      cacheRead: parsePriceInput(cacheRead),
      cacheWrite: parsePriceInput(cacheWrite),
    };
    for (const [label, value] of [
      ['输入', prices.input],
      ['输出', prices.output],
      ['缓存读取', prices.cacheRead],
      ['缓存写入', prices.cacheWrite],
    ] as const) {
      if (value === null) {
        setLocalError(`${label}单价必须是 0 或正数，单位是美元 / 百万 Token`);
        return;
      }
    }
    const multiplierBasisPoints = parseMultiplier(multiplier);
    if (multiplierBasisPoints === null) {
      setLocalError('倍率必须是正数，例如 1 或 1.15');
      return;
    }
    const priorityValue = Number.parseInt(priority.trim() === '' ? '0' : priority.trim(), 10);
    if (!Number.isInteger(priorityValue)) {
      setLocalError('优先级必须是整数');
      return;
    }

    setBusy(true);
    setLocalError(null);
    const ok = await onSubmit({
      modelPattern: trimmed,
      pricingStyle: style,
      inputPricePerMMicros: prices.input ?? 0,
      outputPricePerMMicros: prices.output ?? 0,
      cacheReadPricePerMMicros: prices.cacheRead ?? 0,
      cacheWritePricePerMMicros: prices.cacheWrite ?? 0,
      multiplierBasisPoints,
      priority: priorityValue,
    });
    setBusy(false);
    if (ok) onClose();
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={editing ? '编辑定价' : '新增定价'}
      size="lg"
      hint="单价单位为美元 / 百万 Token"
      secondary={{ label: '取消', onClick: onClose }}
      primary={{
        label: editing ? '保存并替换' : '新增规则',
        busy,
        busyLabel: '正在保存',
        onClick: () => void submit(),
      }}
    >
      <TextField
        label="模型匹配式"
        value={pattern}
        placeholder="例如 deepseek-* 或 gpt-5"
        hint="支持 * 与 ? 通配符，匹配不区分大小写；同优先级下字面字符更多的规则胜出"
        onChange={setPattern}
      />

      <div className="space-y-1.5">
        <Text tone="secondary" size="xs" className="block">
          计费风格
        </Text>
        <SegmentedControl
          label="计费风格"
          value={style}
          onChange={setStyle}
          options={[
            { value: 'openai', label: 'OpenAI 风格' },
            { value: 'claude', label: 'Claude 风格' },
          ]}
        />
        <Text tone="tertiary" size="2xs" className="block">
          {style === 'openai'
            ? 'OpenAI 风格：缓存写入不单独计价，缓存写入单价按 0 计'
            : 'Claude 风格：缓存写入按独立单价计价'}
        </Text>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <TextField label="输入单价" value={input} placeholder="0.8" onChange={setInput} />
        <TextField label="输出单价" value={output} placeholder="4" onChange={setOutput} />
        <TextField label="缓存读取单价" value={cacheRead} placeholder="0.08" onChange={setCacheRead} />
        <TextField label="缓存写入单价" value={cacheWrite} placeholder="1" onChange={setCacheWrite} />
        <TextField
          label="费率倍率"
          value={multiplier}
          placeholder="1.0"
          hint="1 表示按上面的单价原价计费"
          onChange={setMultiplier}
        />
        <TextField
          label="优先级"
          value={priority}
          placeholder="0"
          hint="数值越大越优先命中"
          onChange={setPriority}
        />
      </div>

      {editing && (
        <Text tone="tertiary" size="meta" className="block">
          保存时会先写入新规则，成功后再删除原规则（{rule?.modelPattern}）：
          中途失败时原规则保持不变，不会有「这个模型没有价格」的空档。
        </Text>
      )}

      {localError !== null && (
        <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={localError} />
      )}
    </Dialog>
  );
};
