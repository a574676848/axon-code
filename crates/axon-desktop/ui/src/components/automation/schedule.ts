/** 计划表单只负责表达式与控件互转，调度规则由 Gateway 校验。 */
import { formatDateTime } from '../pageFormat';
import { describeCronSchedule, describeLooseExpression } from './scheduleDescribe';

/** 日历范围：与日期方式正交，两者组合即任意「哪天」语义。 */
export type CalendarScope = '' | 'workday' | 'weekday' | 'weekend' | 'holiday';
/** 日期方式：每天、每周几、每月几号、每年几月几号。 */
export type DateMode = 'none' | 'weekly' | 'monthly' | 'yearly';
export type ScheduleKind = 'once' | 'interval' | 'cron' | 'custom';

export interface ScheduleForm {
    kind: ScheduleKind;
    date: string;
    time: string;
    amount: string;
    unit: string;
    calendar: CalendarScope;
    dateMode: DateMode;
    weekdays: string[];
    monthDay: string;
    month: string;
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
const MAX_MONTH = 12;
const MILLISECONDS_PER_SECOND = 1_000;
const MILLISECONDS_PER_MINUTE = MILLISECONDS_PER_SECOND * 60;
const MILLISECONDS_PER_HOUR = MILLISECONDS_PER_MINUTE * 60;
const MILLISECONDS_PER_DAY = MILLISECONDS_PER_HOUR * HOURS_PER_DAY;

export const WEEKDAYS = ['周日', '周一', '周二', '周三', '周四', '周五', '周六'];
export const INTERVAL_OPTIONS = [
    { value: 'm', label: '分钟' }, { value: 'h', label: '小时' },
    { value: 'd', label: '天' }, { value: 's', label: '秒' }, { value: 'ms', label: '毫秒' },
];
export const KIND_OPTIONS = [
    { value: 'once', label: '一次执行' },
    { value: 'cron', label: '按计划执行' },
    { value: 'interval', label: '固定间隔' },
];
export const CALENDAR_OPTIONS = [
    { value: '', label: '不限日历' },
    { value: 'workday', label: '工作日' },
    { value: 'weekday', label: '周一到周五' },
    { value: 'weekend', label: '周末' },
    { value: 'holiday', label: '法定节假日' },
];
export const DATE_MODE_OPTIONS = [
    { value: 'none', label: '每天' },
    { value: 'weekly', label: '每周' },
    { value: 'monthly', label: '每月' },
    { value: 'yearly', label: '每年' },
];

export function emptySchedule(): ScheduleForm {
    const now = new Date();
    const date = [now.getFullYear(), String(now.getMonth() + 1).padStart(2, '0'),
        String(now.getDate()).padStart(2, '0')].join('-');
    return {
        kind: 'cron',
        date,
        time: '09:00',
        amount: '1',
        unit: 'h',
        calendar: '',
        dateMode: 'none',
        weekdays: [],
        monthDay: '1',
        month: '1',
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
 * 只有能被控件无损重建的表达式才给出结构化结果，其余一律回落 `custom` 并保留原文，
 * 避免打开编辑器再保存时改掉用户本来的计划。
 */
export function parseSchedule(value: string): ScheduleForm {
    const base = { ...emptySchedule(), original: value };
    const trimmed = value.trim();
    const once = /^once:(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2})$/.exec(trimmed);
    if (once) return { ...base, kind: 'once', date: once[1], time: once[2] };
    const interval = /^interval:(\d+)(ms|s|m|h)$/.exec(trimmed);
    if (interval) return { ...base, kind: 'interval', amount: interval[1], unit: interval[2] };

    const prefixed = /^(workday|weekday|weekend|holiday):(.+)$/.exec(trimmed);
    const cron = parseCronFields((prefixed ? prefixed[2] : trimmed).split(/\s+/));
    if (!cron) return { ...base, kind: 'custom' };

    const dateMode: DateMode = cron.month !== null
        ? 'yearly'
        : cron.day !== null
            ? 'monthly'
            : cron.weekdays !== null
                ? 'weekly'
                : 'none';
    return {
        ...base,
        kind: 'cron',
        time: `${pad(cron.hourStart)}:${pad(cron.minuteValue ?? 0)}`,
        calendar: prefixed ? (prefixed[1] as CalendarScope) : '',
        dateMode,
        weekdays: cron.weekdays ? cron.weekdays.map(String) : [],
        monthDay: cron.day !== null ? String(cron.day) : base.monthDay,
        month: cron.month !== null ? String(cron.month) : base.month,
        span: cron.minuteStep !== null || cron.hourStart !== cron.hourEnd,
        spanStart: pad(cron.hourStart),
        spanEnd: pad(cron.hourEnd),
        spanEvery: String(cron.minuteStep ?? cron.hourStep),
        spanUnit: cron.minuteStep !== null ? 'minute' : 'hour',
    };
}

export function encodeSchedule(form: ScheduleForm): string {
    if (form.kind === 'custom') return form.original;
    if (form.kind === 'once') return `once:${form.date}T${form.time.length === 5 ? `${form.time}:00` : form.time}`;
    if (form.kind === 'interval') {
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
    const dayField = form.dateMode === 'monthly' || form.dateMode === 'yearly' ? form.monthDay : '*';
    const monthField = form.dateMode === 'yearly' ? form.month : '*';
    const weekdayField = form.dateMode === 'weekly' ? encodeWeekdays(form.weekdays) : '*';
    const prefix = form.calendar ? `${form.calendar}:` : '';
    return `${prefix}${minuteField} ${hourField} ${dayField} ${monthField} ${weekdayField}`;
}

export function scheduleError(form: ScheduleForm, now = Date.now()): string | null {
    if (form.kind === 'custom') return null;
    if (form.kind === 'interval') {
        const amount = Number(form.amount);
        return Number.isSafeInteger(amount) && amount > 0 && Number.isSafeInteger(amount * HOURS_PER_DAY)
            ? null : '间隔必须是大于零的整数';
    }
    if (!/^\d{2}:\d{2}(:\d{2})?$/.test(form.time)) return '请选择执行时间';
    const [hour, minute, second = 0] = form.time.split(':').map(Number);
    if (hour > MAX_HOUR || minute > MAX_MINUTE || second > MAX_MINUTE) return '执行时间无效';
    if (form.kind === 'once') {
        const at = new Date(`${form.date}T${form.time}`).getTime();
        if (!Number.isFinite(at)) return '请选择执行日期';
        if (at <= now) return '一次执行的日期和时间必须晚于当前时间';
        return null;
    }
    if (form.dateMode === 'monthly' || form.dateMode === 'yearly') {
        const day = Number(form.monthDay);
        if (!Number.isInteger(day) || day < 1 || day > MAX_MONTH_DAY) return '每月执行日必须在 1 到 31 之间';
    }
    if (form.dateMode === 'yearly') {
        const month = Number(form.month);
        if (!Number.isInteger(month) || month < 1 || month > MAX_MONTH) return '每年执行的月份必须在 1 到 12 之间';
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
    if (form.kind === 'custom') return describeLooseExpression(value) ?? `自定义计划：${value}`;
    if (form.kind === 'once') {
        const at = new Date(`${form.date}T${form.time}`).getTime();
        const clock = Number.isFinite(at) ? formatDateTime(at) : `${form.date} ${form.time}`;
        return `${clock} 执行一次`;
    }
    if (form.kind === 'interval') return `每 ${formatInterval(form.amount, form.unit)}`;
    return describeCronSchedule(form);
}

interface ParsedCron {
    minuteValue: number | null;
    minuteStep: number | null;
    hourStart: number;
    hourEnd: number;
    hourStep: number;
    day: number | null;
    month: number | null;
    weekdays: number[] | null;
}

/**
 * 解析五段 cron。返回 `null` 表示控件无法无损表达，调用方必须回落 `custom`。
 *
 * 不可表达的形态：分钟或小时为 `*`、分钟步长与小时步长并存、月份有值但日期不限、
 * 日与星期同时限定。它们都真实存在于网关侧，静默改写会直接改变触发频率。
 */
function parseCronFields(fields: string[]): ParsedCron | null {
    if (fields.length !== 5) return null;
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
        const hours = fields[1].split(',').map(Number);
        const step = hours[1] - hours[0];
        if (step < 1) return null;
        for (let index = 2; index < hours.length; index += 1) {
            if (hours[index] - hours[index - 1] !== step) return null;
        }
        hourStart = hours[0];
        hourEnd = hours[hours.length - 1];
        hourStep = step;
    } else {
        return null;
    }
    if (hourStart > MAX_HOUR || hourEnd > MAX_HOUR || hourStart > hourEnd) return null;
    if (hourStep < 1 || hourStep > HOURS_PER_DAY) return null;
    if (hourStep > 1 && hourStart === hourEnd) return null;
    if (minuteStep !== null && hourStep > 1) return null;

    let day: number | null = null;
    if (fields[2] !== '*' && fields[2] !== '?') {
        if (!/^\d+$/.test(fields[2])) return null;
        day = Number(fields[2]);
        if (day < 1 || day > MAX_MONTH_DAY) return null;
    }

    let month: number | null = null;
    if (fields[3] !== '*' && fields[3] !== '?') {
        if (!/^\d+$/.test(fields[3])) return null;
        month = Number(fields[3]);
        if (month < 1 || month > MAX_MONTH) return null;
    }
    if (month !== null && day === null) return null;

    let weekdays: number[] | null = null;
    if (fields[4] !== '*' && fields[4] !== '?') {
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

    return { minuteValue, minuteStep, hourStart, hourEnd, hourStep, day, month, weekdays };
}

function encodeWeekdays(weekdays: string[]): string {
    const sorted = [...new Set(weekdays.map(Number))].sort((left, right) => left - right);
    return sorted.length > 0 ? sorted.join(',') : '*';
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
