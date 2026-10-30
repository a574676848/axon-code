/** 计划表单只负责表达式与日历控件互转，调度规则由 Gateway 校验。 */
import { formatDateTime } from '../pageFormat';

export type ScheduleMode =
    | 'once'
    | 'daily'
    | 'weekly'
    | 'monthly'
    | 'workday'
    | 'weekday'
    | 'weekend'
    | 'holiday'
    | 'interval'
    | 'custom';

export interface ScheduleForm {
    mode: ScheduleMode;
    date: string;
    time: string;
    weekdays: string[];
    day: string;
    amount: string;
    unit: string;
    span: boolean;
    spanStart: string;
    spanEnd: string;
    spanEvery: string;
    spanUnit: 'hour' | 'minute';
    original: string;
}

const HOURS_PER_DAY = 24;
const MINUTES_PER_HOUR = 60;
const MAX_HOUR = HOURS_PER_DAY - 1;
const MAX_MINUTE = MINUTES_PER_HOUR - 1;
const MAX_MONTH_DAY = 31;
const MILLISECONDS_PER_SECOND = 1_000;
const MILLISECONDS_PER_MINUTE = MILLISECONDS_PER_SECOND * 60;
const MILLISECONDS_PER_HOUR = MILLISECONDS_PER_MINUTE * 60;
const MILLISECONDS_PER_DAY = MILLISECONDS_PER_HOUR * HOURS_PER_DAY;
const WEEKDAY_COUNT = 7;
const WORKDAY_WEEKDAYS = '1,2,3,4,5';

export const WEEKDAYS = ['周日', '周一', '周二', '周三', '周四', '周五', '周六'];
export const SCHEDULE_OPTIONS = [
    { value: 'once', label: '一次执行' },
    { value: 'daily', label: '每天执行' },
    { value: 'weekly', label: '每周执行' },
    { value: 'monthly', label: '每月执行' },
    { value: 'workday', label: '每个工作日' },
    { value: 'weekday', label: '周一到周五' },
    { value: 'weekend', label: '每个周末' },
    { value: 'holiday', label: '每个法定节假日' },
    { value: 'interval', label: '固定间隔' },
];
export const INTERVAL_OPTIONS = [
    { value: 'm', label: '分钟' }, { value: 'h', label: '小时' },
    { value: 'd', label: '天' }, { value: 's', label: '秒' }, { value: 'ms', label: '毫秒' },
];
export const CALENDAR_MODES: ScheduleMode[] = ['workday', 'weekday', 'weekend', 'holiday'];

export function emptySchedule(): ScheduleForm {
    const now = new Date();
    const date = [now.getFullYear(), String(now.getMonth() + 1).padStart(2, '0'),
        String(now.getDate()).padStart(2, '0')].join('-');
    return {
        mode: 'daily',
        date,
        time: '09:00',
        weekdays: ['1'],
        day: '1',
        amount: '1',
        unit: 'h',
        span: false,
        spanStart: '09',
        spanEnd: '18',
        spanEvery: '1',
        spanUnit: 'hour',
        original: '',
    };
}

/**
 * 解析计划表达式。
 *
 * 只有能被表单无损重建的表达式才返回结构化结果，其余一律回落 `custom` 并保留原文，
 * 避免打开编辑器再保存时改掉用户本来的计划。
 */
export function parseSchedule(value: string): ScheduleForm {
    const base = { ...emptySchedule(), original: value };
    const trimmed = value.trim();
    const once = /^once:(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2})$/.exec(trimmed);
    if (once) return { ...base, mode: 'once', date: once[1], time: once[2] };
    const interval = /^interval:(\d+)(ms|s|m|h)$/.exec(trimmed);
    if (interval) return { ...base, mode: 'interval', amount: interval[1], unit: interval[2] };

    const prefixed = /^(workday|weekday|weekend|holiday):(.+)$/.exec(trimmed);
    const cron = parseCronFields((prefixed ? prefixed[2] : trimmed).split(/\s+/));
    if (!cron) return { ...base, mode: 'custom' };

    const form: ScheduleForm = {
        ...base,
        time: `${pad(cron.hourStart)}:${pad(cron.minuteValue ?? 0)}`,
        span: cron.minuteStep !== null || cron.hourStart !== cron.hourEnd,
        spanStart: pad(cron.hourStart),
        spanEnd: pad(cron.hourEnd),
        spanEvery: String(cron.minuteStep ?? cron.hourStep),
        spanUnit: cron.minuteStep !== null ? 'minute' : 'hour',
    };
    if (prefixed) {
        // 日历前缀与日、星期限定组合时表单无法同时表达，回落自定义以免丢字段。
        if (cron.day !== null || cron.weekdays !== null) return { ...base, mode: 'custom' };
        return { ...form, mode: prefixed[1] as ScheduleMode };
    }
    if (cron.day !== null) return { ...form, mode: 'monthly', day: String(cron.day) };
    if (cron.weekdays) {
        return { ...form, mode: 'weekly', weekdays: cron.weekdays.map(String) };
    }
    return { ...form, mode: 'daily' };
}

export function encodeSchedule(form: ScheduleForm): string {
    if (form.mode === 'custom') return form.original;
    if (form.mode === 'once') return `once:${form.date}T${form.time.length === 5 ? `${form.time}:00` : form.time}`;
    if (form.mode === 'interval') {
        return `interval:${form.unit === 'd' ? Number(form.amount) * HOURS_PER_DAY : form.amount}${form.unit === 'd' ? 'h' : form.unit}`;
    }
    const [hour, minute] = form.time.split(':').map(Number);
    let minuteField = String(minute);
    let hourField = String(hour);
    if (form.span) {
        const start = Number(form.spanStart);
        const end = Number(form.spanEnd);
        const every = Math.max(1, Math.trunc(Number(form.spanEvery)) || 1);
        if (form.spanUnit === 'minute') {
            minuteField = `*/${every}`;
            hourField = start === end ? String(start) : `${start}-${end}`;
        } else {
            hourField = start === end ? String(start) : every > 1 ? `${start}-${end}/${every}` : `${start}-${end}`;
        }
    }
    const prefix = CALENDAR_MODES.includes(form.mode) ? `${form.mode}:` : '';
    const dayField = form.mode === 'monthly' ? form.day : '*';
    const weekdayField = form.mode === 'weekly' ? encodeWeekdays(form.weekdays) : '*';
    return `${prefix}${minuteField} ${hourField} ${dayField} * ${weekdayField}`;
}

export function scheduleError(form: ScheduleForm, now = Date.now()): string | null {
    if (form.mode === 'custom') return null;
    if (form.mode === 'interval') {
        const amount = Number(form.amount);
        return Number.isSafeInteger(amount) && amount > 0 && Number.isSafeInteger(amount * HOURS_PER_DAY)
            ? null : '间隔必须是大于零的整数';
    }
    if (!/^\d{2}:\d{2}(:\d{2})?$/.test(form.time)) return '请选择执行时间';
    const [hour, minute, second = 0] = form.time.split(':').map(Number);
    if (hour > MAX_HOUR || minute > MAX_MINUTE || second > MAX_MINUTE) return '执行时间无效';
    if (form.mode === 'once') {
        const at = new Date(`${form.date}T${form.time}`).getTime();
        if (!Number.isFinite(at)) return '请选择执行日期';
        if (at <= now) return '一次执行的日期和时间必须晚于当前时间';
        return null;
    }
    if (form.span) {
        const start = Number(form.spanStart);
        const end = Number(form.spanEnd);
        if (!Number.isInteger(start) || !Number.isInteger(end) || start > end) return '时段起始小时不能晚于结束小时';
        const every = Number(form.spanEvery);
        if (!Number.isSafeInteger(every) || every <= 0) return '时段间隔必须是大于零的整数';
        if (form.spanUnit === 'minute' && every > MAX_MINUTE) return '分钟间隔不能超过 59';
        if (form.spanUnit === 'hour' && every > HOURS_PER_DAY) return '小时间隔不能超过 24';
    }
    return null;
}

export function scheduleText(value: string): string {
    const form = parseSchedule(value);
    switch (form.mode) {
        case 'once': {
            const at = new Date(`${form.date}T${form.time}`).getTime();
            const clock = Number.isFinite(at) ? formatDateTime(at) : `${form.date} ${form.time}`;
            return `${clock} 执行一次`;
        }
        case 'interval': return `每 ${formatInterval(form.amount, form.unit)}`;
        case 'custom': return `自定义计划：${value}`;
        default: return describeCron(form);
    }
}

function describeCron(form: ScheduleForm): string {
    const base = form.mode === 'workday' ? '每个工作日'
        : form.mode === 'weekday' ? '周一到周五'
            : form.mode === 'weekend' ? '每个周末'
                : form.mode === 'holiday' ? '每个法定节假日'
                    : form.mode === 'weekly' ? describeWeekdays(form.weekdays)
                        : form.mode === 'monthly' ? `每月 ${form.day} 日`
                            : '每天';
    if (!form.span) return `${base} ${form.time}`;
    const start = Number(form.spanStart);
    const end = Number(form.spanEnd);
    const every = Number(form.spanEvery) || 1;
    const cadence = form.spanUnit === 'minute'
        ? (every > 1 ? `每 ${every} 分钟` : '每分钟')
        : (every > 1 ? `每 ${every} 小时` : '每小时');
    const minute = Number(form.time.split(':')[1] ?? '0');
    const offset = minute > 0 ? `的第 ${minute} 分` : '';
    if (start === end) {
        return form.spanUnit === 'minute' ? `${base} ${start} 点起${cadence}` : `${base} ${start} 点`;
    }
    return `${base} ${start} 点到 ${end} 点${cadence}${offset}`;
}

function describeWeekdays(weekdays: string[]): string {
    const sorted = [...weekdays].map(Number).sort((left, right) => left - right);
    if (sorted.length === 0 || sorted.length === WEEKDAY_COUNT) return '每天';
    if (sorted.join(',') === WORKDAY_WEEKDAYS) return '每个工作日';
    return `每${sorted.map((day) => WEEKDAYS[day]).join('、')}`;
}

function encodeWeekdays(weekdays: string[]): string {
    const sorted = [...new Set(weekdays.map(Number))].sort((left, right) => left - right);
    return sorted.length > 0 ? sorted.join(',') : '*';
}

interface ParsedCron {
    minuteValue: number | null;
    minuteStep: number | null;
    hourStart: number;
    hourEnd: number;
    hourStep: number;
    day: number | null;
    weekdays: number[] | null;
}

/**
 * 解析五段 cron。返回 `null` 表示表单无法无损表达，调用方必须回落 `custom`。
 *
 * 不可表达的形态：分钟或小时为 `*`、月份非 `*`、日与星期同时限定、分钟步长与小时步长并存、
 * 时段内分钟非零。它们都真实存在于网关侧，静默改写会直接改变触发频率。
 */
function parseCronFields(fields: string[]): ParsedCron | null {
    if (fields.length !== 5 || fields[3] !== '*') return null;
    if (fields[0] === '*' || fields[0] === '?') return null;
    if (fields[1] === '*' || fields[1] === '?') return null;

    let minuteValue: number | null = null;
    let minuteStep: number | null = null;
    const minuteStepMatch = /^\*\/(\d+)$/.exec(fields[0]);
    if (minuteStepMatch) {
        minuteStep = Number(minuteStepMatch[1]);
        if (minuteStep < 1 || minuteStep > MAX_MINUTE) return null;
    } else if (/^\d+$/.test(fields[0])) {
        minuteValue = Number(fields[0]);
        if (minuteValue > MAX_MINUTE) return null;
    } else {
        return null;
    }

    let hourStart: number;
    let hourEnd: number;
    let hourStep = 1;
    const range = /^(\d+)-(\d+)(?:\/(\d+))?$/.exec(fields[1]);
    const single = /^\d+$/.exec(fields[1]);
    if (range) {
        hourStart = Number(range[1]);
        hourEnd = Number(range[2]);
        hourStep = range[3] ? Number(range[3]) : 1;
    } else if (single) {
        hourStart = Number(single[0]);
        hourEnd = hourStart;
    } else if (/^\d+(,\d+)+$/.test(fields[1])) {
        // 连续递增的逗号列表与区间等价，归一化后交给时段控件表达
        const hours = fields[1].split(',').map(Number);
        for (let index = 1; index < hours.length; index += 1) {
            if (hours[index] !== hours[index - 1] + 1) return null;
        }
        hourStart = hours[0];
        hourEnd = hours[hours.length - 1];
    } else {
        return null;
    }
    if (hourStart > MAX_HOUR || hourEnd > MAX_HOUR || hourStart > hourEnd) return null;
    if (hourStep < 1 || hourStep > HOURS_PER_DAY) return null;
    if (hourStep > 1 && hourStart === hourEnd) return null;
    if (minuteStep !== null && hourStep > 1) return null;

    let day: number | null = null;
    if (fields[2] !== '*') {
        if (!/^\d+$/.test(fields[2])) return null;
        day = Number(fields[2]);
        if (day < 1 || day > MAX_MONTH_DAY) return null;
    }

    let weekdays: number[] | null = null;
    if (fields[4] !== '*') {
        const values: number[] = [];
        for (const part of fields[4].split(',')) {
            const weekdayRange = /^(\d)-(\d)$/.exec(part);
            if (weekdayRange) {
                const from = Number(weekdayRange[1]);
                const to = Number(weekdayRange[2]);
                if (from > to) return null;
                for (let weekday = from; weekday <= to; weekday += 1) values.push(weekday);
            } else if (/^\d$/.test(part)) {
                values.push(Number(part));
            } else {
                return null;
            }
        }
        if (values.length === 0 || values.some((value) => value > 6)) return null;
        weekdays = [...new Set(values)].sort((left, right) => left - right);
    }
    if (day !== null && weekdays !== null) return null;

    return { minuteValue, minuteStep, hourStart, hourEnd, hourStep, day, weekdays };
}

function pad(value: number): string {
    return String(value).padStart(2, '0');
}

function formatInterval(amountText: string, unit: string): string {
    const amount = Number(amountText);
    if (!Number.isFinite(amount) || amount < 0) {
        return `${amountText} ${INTERVAL_OPTIONS.find((option) => option.value === unit)?.label ?? unit}`;
    }
    const milliseconds = unit === 'ms'
        ? amount
        : unit === 's'
            ? amount * MILLISECONDS_PER_SECOND
            : unit === 'm'
                ? amount * MILLISECONDS_PER_MINUTE
                : unit === 'h'
                    ? amount * MILLISECONDS_PER_HOUR
                    : amount * MILLISECONDS_PER_DAY;
    const units = [
        { size: MILLISECONDS_PER_DAY, label: '天' },
        { size: MILLISECONDS_PER_HOUR, label: '小时' },
        { size: MILLISECONDS_PER_MINUTE, label: '分钟' },
        { size: MILLISECONDS_PER_SECOND, label: '秒' },
    ];
    const readable = units.find(({ size }) => milliseconds >= size && milliseconds % size === 0);
    if (readable) return `${milliseconds / readable.size} ${readable.label}`;
    if (milliseconds < MILLISECONDS_PER_SECOND) return `${milliseconds} 毫秒`;
    return `${(milliseconds / MILLISECONDS_PER_SECOND).toLocaleString('zh-CN', { maximumFractionDigits: 2 })} 秒`;
}
