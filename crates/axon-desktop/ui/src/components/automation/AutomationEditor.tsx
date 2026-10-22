import React from "react";
import { CheckIcon, CloseIcon } from "../../icons";
import { Button, Checkbox, Input, Textarea } from "../ui/controls";
import { FormField, ToggleField } from "../ui/surfaces";
import { SkillsMultiSelect } from "./SkillsMultiSelect";
import { ScheduleFields } from "./ScheduleFields";
import type { ScheduleForm } from "./schedule";

/** 编辑器持有的全部字段；页面在打开编辑时用它快照任务。 */
export type FormState = {
  name: string;
  schedule: string;
  prompt: string;
  recipientId: string;
  threadId: string;
  workdir: string;
  skills: string[];
  noAgent: boolean;
  script: string;
};

/** 新建任务时的空表单。 */
export const EMPTY_FORM: FormState = {
  name: "",
  schedule: "0 9 * * *",
  prompt: "",
  recipientId: "",
  threadId: "",
  workdir: "",
  skills: [],
  noAgent: false,
  script: "",
};

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
    <FormField label="投递目标">
      <Input
        value={form.recipientId}
        onChange={(event) => onFormChange({ recipientId: event.target.value })}
        placeholder="可选，留空则仅在会话中查看结果"
        aria-label="投递目标 ID"
      />
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
