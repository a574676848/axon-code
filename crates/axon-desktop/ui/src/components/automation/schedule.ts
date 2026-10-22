/** 计划表单只负责表达式与日历控件互转，调度规则由 Gateway 校验。 */
export type ScheduleMode = 'once' | 'daily' | 'weekly' | 'monthly' | 'interval' | 'custom';
export interface ScheduleForm {
    mode: ScheduleMode;
    date: string;
    time: string;
    weekday: string;
    day: string;
    amount: string;
    unit: string;
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
export const WEEKDAYS = ['周日', '周一', '周二', '周三', '周四', '周五', '周六'];
export const SCHEDULE_OPTIONS = [
    { value: 'once', label: '一次执行' },
    { value: 'daily', label: '每天执行' },
    { value: 'weekly', label: '每周执行' },
    { value: 'monthly', label: '每月执行' },
    { value: 'interval', label: '固定间隔' },
];
export const INTERVAL_OPTIONS = [
    { value: 'm', label: '分钟' }, { value: 'h', label: '小时' },
    { value: 'd', label: '天' }, { value: 's', label: '秒' }, { value: 'ms', label: '毫秒' },
];

export function emptySchedule(): ScheduleForm {
    const now = new Date();
    const date = [now.getFullYear(), String(now.getMonth() + 1).padStart(2, '0'),
        String(now.getDate()).padStart(2, '0')].join('-');
    return { mode: 'daily', date, time: '09:00', weekday: '1', day: '1', amount: '1', unit: 'h', original: '' };
}

export function parseSchedule(value: string): ScheduleForm {
    const base = { ...emptySchedule(), original: value };
    const once = /^once:(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2})$/.exec(value.trim());
    if (once) return { ...base, mode: 'once', date: once[1], time: once[2] };
    const interval = /^interval:(\d+)(ms|s|m|h)$/.exec(value.trim());
    if (interval) return { ...base, mode: 'interval', amount: interval[1], unit: interval[2] };
    const fields = value.trim().split(/\s+/);
    if (fields.length !== 5 || !/^\d+$/.test(fields[0]) || !/^\d+$/.test(fields[1]) || fields[3] !== '*') {
        return { ...base, mode: 'custom' };
    }
    if (Number(fields[0]) > MAX_MINUTE || Number(fields[1]) > MAX_HOUR) return { ...base, mode: 'custom' };
    const time = `${fields[1].padStart(2, '0')}:${fields[0].padStart(2, '0')}`;
    if (fields[2] === '*' && fields[4] === '*') return { ...base, mode: 'daily', time };
    if (fields[2] === '*' && /^[0-6]$/.test(fields[4])) return { ...base, mode: 'weekly', time, weekday: fields[4] };
    if (fields[4] === '*' && /^\d+$/.test(fields[2]) && Number(fields[2]) >= 1 && Number(fields[2]) <= MAX_MONTH_DAY) {
        return { ...base, mode: 'monthly', time, day: fields[2] };
    }
    return { ...base, mode: 'custom' };
}

export function encodeSchedule(form: ScheduleForm): string {
    if (form.mode === 'custom') return form.original;
    if (form.mode === 'once') return `once:${form.date}T${form.time.length === 5 ? `${form.time}:00` : form.time}`;
    if (form.mode === 'interval') {
        return `interval:${form.unit === 'd' ? Number(form.amount) * HOURS_PER_DAY : form.amount}${form.unit === 'd' ? 'h' : form.unit}`;
    }
    const [hour, minute] = form.time.split(':').map(Number);
    return `${minute} ${hour} ${form.mode === 'monthly' ? form.day : '*'} * ${form.mode === 'weekly' ? form.weekday : '*'}`;
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
    }
    return null;
}

export function scheduleText(value: string): string {
    const form = parseSchedule(value);
    switch (form.mode) {
        case 'once': return `${form.date} ${form.time} 执行一次`;
        case 'daily': return `每天 ${form.time}`;
        case 'weekly': return `每${WEEKDAYS[Number(form.weekday)]} ${form.time}`;
        case 'monthly': return `每月 ${form.day} 日 ${form.time}`;
        case 'interval': return `每 ${formatInterval(form.amount, form.unit)}`;
        default: return `自定义计划：${value}`;
    }
}

/**
 * 设置页「模型定价」自动同步状态行的时刻读数：本地完整时刻，带秒。
 *
 * 自动化任务页统一改用 `pageFormat.formatDateTime`（同年 `10/21 17:14`，跨年带年份），
 * 因此这里的带秒写法只服务于那一条既有状态行，不再作为任务页的口径。
 */
export function cronTimeText(value?: number | null): string {
    return value == null ? '未安排' : new Date(value).toLocaleString();
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
