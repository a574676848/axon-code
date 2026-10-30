import React from "react";
import { CheckIcon, CloseIcon } from "../../icons";
import { Button, Checkbox, Input, Textarea } from "../ui/controls";
import { FormField, Text, ToggleField } from "../ui/surfaces";
import { SkillsMultiSelect } from "./SkillsMultiSelect";
import { ScheduleFields } from "./ScheduleFields";
import type { ScheduleForm } from "./schedule";
import type { FormState } from "./jobForm";

/**
 * 任务编辑表单：字段标签 12/16，值 14/20，动作按钮统一 28。
 * 保存前的校验由页面统一处理，这里只负责收集输入。
 */
export const AutomationEditor: React.FC<{
  form: FormState;
  scheduleForm: ScheduleForm;
  busy: boolean;
  onFormChange: (patch: Partial<FormState>) => void;
  onScheduleChange: (value: ScheduleForm) => void;
  onCancel: () => void;
  onSave: () => void;
}> = ({ form, scheduleForm, busy, onFormChange, onScheduleChange, onCancel, onSave }) => (
  <div className="space-y-4">
    <FormField label="任务名称">
      <Input
        value={form.name}
        onChange={(event) => onFormChange({ name: event.target.value })}
        placeholder="例如：每日同步"
        aria-label="任务名称"
      />
    </FormField>
    <ScheduleFields value={scheduleForm} onChange={onScheduleChange} />
    <FormField label="任务提示词">
      <Textarea
        className="min-h-24"
        value={form.prompt}
        onChange={(event) => onFormChange({ prompt: event.target.value })}
        placeholder="让 Agent 执行的内容"
        aria-label="任务提示词"
      />
    </FormField>
    <FormField label="投递目标（feishu:<chat_id> 或 session:<channel>:<destination>）">
      <Input
        value={form.wake}
        onChange={(event) => onFormChange({ wake: event.target.value })}
        placeholder="feishu:oc_xxx 或 session:desktop:workbench-1"
        aria-label="投递目标 wake"
      />
      <Text size="xs" tone="tertiary" className="block break-words">
        channel 只能是 feishu 或 desktop。留空则不投递：任务执行完只记录运行事实。
      </Text>
    </FormField>
    <FormField label="Skills">
      <SkillsMultiSelect
        value={form.skills}
        onChange={(skills) => onFormChange({ skills })}
      />
    </FormField>
    <ToggleField
      label="仅运行脚本"
      description="不调用 Agent，直接执行脚本内容。"
      control={
        <Checkbox
          checked={form.noAgent}
          onChange={(next) => onFormChange({ noAgent: next })}
          label="仅运行脚本"
        />
      }
    />
    {form.noAgent && (
      <FormField label="脚本">
        <Textarea
          className="min-h-24 font-mono"
          value={form.script}
          onChange={(event) => onFormChange({ script: event.target.value })}
          aria-label="脚本"
        />
      </FormField>
    )}
    <div className="flex items-center justify-end gap-2 pt-1">
      <Button variant="ghost" size="sm" disabled={busy} onClick={onCancel}>
        <CloseIcon size={14} />
        取消
      </Button>
      <Button
        variant="primary"
        size="sm"
        disabled={busy || !form.name.trim()}
        onClick={onSave}
      >
        <CheckIcon size={14} />
        {busy ? "保存中" : "保存任务"}
      </Button>
    </div>
  </div>
);
