import type { CronJob } from "../../types";

/*
 * 自动化任务的表单契约：任务快照、表单值与网关请求之间的唯一映射。
 *
 * 投递目标在新模型里只有一个 `wake` 文本字段，取值是 `feishu:<chat_id>` 或
 * `session:<channel>:<destination>`；空串表示不设置目标，请求里落成 `null`，
 * 任务执行完只记录运行事实，不产生任何投递。
 */

/** 编辑器持有的全部字段；页面在打开编辑时用它快照任务。 */
export type FormState = {
  name: string;
  schedule: string;
  prompt: string;
  wake: string;
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
  wake: "",
  workdir: "",
  skills: [],
  noAgent: false,
  script: "",
};

/** 详情里目标缺失时的中文占位。 */
export const WAKE_UNSET_TEXT = "未设置";

/** 网关 `wake` 字段的展示文本；`null` 收成空串。 */
export const wakeText = (wake: string | null | undefined): string =>
  (wake ?? "").trim();

/** 详情展示用文案：没有目标时写「未设置」。 */
export const wakeLabel = (wake: string | null | undefined): string =>
  wakeText(wake) || WAKE_UNSET_TEXT;

/** 表单值转请求值：留空即不设置目标。 */
export const wakeFromForm = (value: string): string | null =>
  value.trim() || null;

/** 表单从任务快照里读取的字段；其余任务事实不参与编辑。 */
export type JobFormSource = Pick<
  CronJob,
  | "name"
  | "schedule"
  | "prompt"
  | "wake"
  | "workdir"
  | "skills"
  | "noAgent"
  | "script"
>;

/** 任务快照转表单。 */
export const formFromJob = (job: JobFormSource): FormState => ({
  name: job.name,
  schedule: job.schedule,
  prompt: job.prompt,
  wake: job.wake ?? "",
  workdir: job.workdir ?? "",
  skills: job.skills,
  noAgent: job.noAgent,
  script: job.script ?? "",
});

/** 创建与更新共用的请求体；更新时另加 replace* 开关。 */
export type CronJobRequest = {
  name: string;
  schedule: string;
  prompt: string;
  wake: string | null;
  workdir: string | null;
  skills: string[];
  noAgent: boolean;
  script: string | null;
};

/** 表单转网关请求体。 */
export const requestFromForm = (form: FormState): CronJobRequest => ({
  name: form.name.trim(),
  schedule: form.schedule,
  prompt: form.prompt,
  wake: wakeFromForm(form.wake),
  workdir: form.workdir.trim() || null,
  skills: form.skills,
  noAgent: form.noAgent,
  script: form.script || null,
});
