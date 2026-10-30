/** 计划表达式的中文描述：表单结构描述与宽松表达式翻译都在这里。 */
import type { DateMode, ScheduleForm } from './schedule';

const WEEKDAY_LABELS = ['周日', '周一', '周二', '周三', '周四', '周五', '周六'];
const WORKDAY_WEEKDAYS = '1,2,3,4,5';
const WEEKDAY_COUNT = 7;

const CALENDAR_LABELS: Record<string, string> = {
    workday: '每个工作日',
    weekday: '周一到周五',
    weekend: '每个周末',
    holiday: '每个法定节假日',
};

const joinSpace = (text: string) => (/^\d/.test(text) ? ` ${text}` : text);

/** 表单结构的描述：日历范围与日期方式是交集，缺省侧直接省略。 */
export function describeCronSchedule(form: ScheduleForm): string {
    const scope = CALENDAR_LABELS[form.calendar] ?? '';
    const rawCore = describeDateCore(form.dateMode, form.weekdays, form.monthDay, form.month);
    // 星期范围与日历范围重合时不再重复描述
    const core = (form.calendar === 'workday' || form.calendar === 'weekday') && rawCore === '每个工作日'
        ? ''
        : rawCore;
    const when = scope && core
        ? `${scope}里的${joinSpace(core)}`
        : scope || (core ? withDateLead(form.dateMode, core) : '每天');
    return `${when} ${describeTime(form.time, form.span, form.spanStart, form.spanEnd, form.spanEvery, form.spanUnit)}`;
}

function describeDateCore(mode: DateMode, weekdays: string[], monthDay: string, month: string): string {
    switch (mode) {
        case 'none': return '';
        case 'weekly': return weekdayNames(weekdays);
        case 'monthly': return `${monthDay} 日`;
        case 'yearly': return `${month} 月 ${monthDay} 日`;
    }
}

function withDateLead(mode: DateMode, core: string): string {
    switch (mode) {
        case 'weekly': return core ? (core.startsWith('每') ? core : `每${core}`) : '';
        case 'monthly': return `每月 ${core}`;
        case 'yearly': return `每年 ${core}`;
        default: return core;
    }
}

function weekdayNames(values: string[]): string {
    const sorted = [...new Set(values.map(Number))].sort((left, right) => left - right);
    if (sorted.length === 0 || sorted.length === WEEKDAY_COUNT) return '';
    if (sorted.join(',') === WORKDAY_WEEKDAYS) return '每个工作日';
    return sorted.map((day) => WEEKDAY_LABELS[day]).join('、');
}

function describeTime(
    time: string,
    span: boolean,
    spanStart: string,
    spanEnd: string,
    spanEvery: string,
    spanUnit: 'hour' | 'minute',
): string {
    if (!span) return time;
    const start = Number(spanStart);
    const end = Number(spanEnd);
    const every = Number(spanEvery) || 1;
    if (spanUnit === 'minute') {
        const cadence = every > 1 ? `每 ${every} 分钟` : '每分钟';
        return start === end ? `${start} 点起${cadence}` : `${start} 点到 ${end} 点${cadence}`;
    }
    const cadence = every > 1 ? `每 ${every} 小时` : '每小时';
    const minute = Number(time.split(':')[1] ?? '0');
    const offset = minute > 0 ? `的第 ${minute} 分` : '';
    return start === end ? `${start} 点${offset}` : `${start} 点到 ${end} 点${cadence}${offset}`;
}

type LooseField =
    | { kind: 'any' }
    | { kind: 'value'; value: number }
    | { kind: 'list'; values: number[] }
    | { kind: 'range'; start: number; end: number; step: number };

/**
 * 宽松解析单个字段：语法合法就给结构，供显示层翻译。
 *
 * 与表单解析不同，这里不要求能被控件重建，因此列表、单值步长等都能落到中文。
 */
function parseLooseField(text: string, min: number, max: number): LooseField | null {
    if (text === '*' || text === '?') return { kind: 'any' };
    const anyStep = /^\*\/(\d+)$/.exec(text);
    if (anyStep) {
        const step = Number(anyStep[1]);
        if (step < 1) return null;
        return { kind: 'range', start: min, end: min + Math.floor((max - min) / step) * step, step };
    }
    const valueStep = /^(\d+)\/(\d+)$/.exec(text);
    if (valueStep) {
        const start = Number(valueStep[1]);
        const step = Number(valueStep[2]);
        if (start < min || start > max || step < 1) return null;
        return { kind: 'range', start, end: start + Math.floor((max - start) / step) * step, step };
    }
    const range = /^(\d+)-(\d+)(?:\/(\d+))?$/.exec(text);
    if (range) {
        const start = Number(range[1]);
        const end = Number(range[2]);
        const step = range[3] ? Number(range[3]) : 1;
        if (start < min || end > max || start > end || step < 1) return null;
        return { kind: 'range', start, end, step };
    }
    if (/^\d+$/.test(text)) {
        const value = Number(text);
        return value >= min && value <= max ? { kind: 'value', value } : null;
    }
    if (/^\d+(,\d+)+$/.test(text)) {
        const values = text.split(',').map(Number);
        return values.some((value) => value < min || value > max) ? null : { kind: 'list', values };
    }
    return null;
}

/** 把任意语法合法的计划翻译成中文；无法翻译时返回 null，由调用方回落原文。 */
export function describeLooseExpression(value: string): string | null {
    const trimmed = value.trim();
    const prefixed = /^(workday|weekday|weekend|holiday):(.+)$/.exec(trimmed);
    const fields = (prefixed ? prefixed[2] : trimmed).split(/\s+/);
    if (fields.length !== 5) return null;
    const minutes = parseLooseField(fields[0], 0, 59);
    const hours = parseLooseField(fields[1], 0, 23);
    const days = parseLooseField(fields[2], 1, 31);
    const months = parseLooseField(fields[3], 1, 12);
    const weekdays = parseLooseField(fields[4], 0, 6);
    if (!minutes || !hours || !days || !months || !weekdays) return null;

    const time = describeLooseTime(minutes, hours);
    if (!time) return null;

    const scope = prefixed ? CALENDAR_LABELS[prefixed[1]] ?? '' : '';
    const monthText = describeMonths(months);
    const dateText = describeLooseDate(days, weekdays, monthText.length > 0, scope.length > 0);
    const pieces: string[] = [];
    if (monthText) {
        const yearly = dateText ? `${monthText} ${dateText}` : monthText;
        pieces.push(scope ? `${scope}里的每年 ${yearly}` : `每年 ${yearly}`);
    } else if (dateText) {
        pieces.push(scope ? `${scope}里的${joinSpace(dateText)}` : dateText);
    } else if (scope) {
        pieces.push(scope);
    }
    const parts = pieces.filter(Boolean);
    if (parts.length === 0) return time;
    return `${parts.join('或')} ${time}`;
}

function describeMonths(field: LooseField): string {
    switch (field.kind) {
        case 'any': return '';
        case 'value': return `${field.value} 月`;
        case 'list': return `${field.values.join('、')} 月`;
        case 'range':
            if (field.step > 1 && field.start === 1) return `每 ${field.step} 个月`;
            return field.step > 1
                ? `${field.start} 月到 ${field.end} 月每 ${field.step} 个月`
                : `${field.start} 月到 ${field.end} 月`;
    }
}

function describeLooseDate(days: LooseField, weekdays: LooseField, withMonth: boolean, bare: boolean): string {
    const dayText = describeDayPart(days, withMonth);
    const weekText = describeWeekPart(weekdays, bare);
    if (dayText && weekText) return `${dayText}或${weekText}`;
    return dayText || weekText;
}

function describeDayPart(field: LooseField, withMonth: boolean): string {
    const prefix = withMonth ? '' : '每月 ';
    switch (field.kind) {
        case 'any': return '';
        case 'value': return `${prefix}${field.value} 日`;
        case 'list': return `${prefix}${field.values.join('、')} 日`;
        case 'range':
            return field.step > 1
                ? `${prefix}${field.start} 日到 ${field.end} 日每 ${field.step} 天`
                : `${prefix}${field.start} 日到 ${field.end} 日`;
    }
}

function describeWeekPart(field: LooseField, bare = false): string {
    const labels = (days: number[]) => days.map((day) => WEEKDAY_LABELS[day]).join('、');
    const every = bare ? '' : '每';
    switch (field.kind) {
        case 'any': return '';
        case 'value': return `${every}${WEEKDAY_LABELS[field.value]}`;
        case 'list': {
            const sorted = [...new Set(field.values)].sort((left, right) => left - right);
            if (sorted.join(',') === WORKDAY_WEEKDAYS) return bare ? '' : '每个工作日';
            return `${every}${labels(sorted)}`;
        }
        case 'range': {
            const days: number[] = [];
            for (let day = field.start; day <= field.end; day += field.step) days.push(day);
            if (days.join(',') === WORKDAY_WEEKDAYS) return bare ? '' : '每个工作日';
            return `${every}${labels(days)}`;
        }
    }
}

function describeLooseTime(minutes: LooseField, hours: LooseField): string {
    const clock = (hour: number, minute: number) => `${pad(hour)}:${pad(minute)}`;
    if (hours.kind === 'any') {
        if (minutes.kind === 'any') return '每分钟';
        if (minutes.kind === 'range' && minutes.start === 0) {
            return minutes.step > 1 ? `每 ${minutes.step} 分钟` : '每分钟';
        }
        if (minutes.kind === 'value') return `每小时的第 ${minutes.value} 分`;
        if (minutes.kind === 'list') return `每小时的第 ${minutes.values.join('、')} 分`;
        return '';
    }
    if (hours.kind === 'value') {
        if (minutes.kind === 'any') return `${hours.value} 点内每分钟`;
        if (minutes.kind === 'value') return clock(hours.value, minutes.value);
        if (minutes.kind === 'range' && minutes.start === 0) {
            return `${hours.value} 点内每 ${minutes.step} 分钟`;
        }
        return '';
    }
    const span = hours.kind === 'range' ? hours : linearHours(hours.values);
    if (!span) return '';
    const cadence = span.step > 1 ? `每 ${span.step} 小时` : '每小时';
    const offset = minutes.kind === 'value' && minutes.value > 0 ? `的第 ${minutes.value} 分` : '';
    return `${span.start} 点到 ${span.end} 点${cadence}${offset}`;
}

/** 等差的小时列表等价于区间加步长，其余列表不翻译。 */
function linearHours(values: number[]): { start: number; end: number; step: number } | null {
    if (values.length < 2) return null;
    const step = values[1] - values[0];
    if (step < 1) return null;
    for (let index = 2; index < values.length; index += 1) {
        if (values[index] - values[index - 1] !== step) return null;
    }
    return { start: values[0], end: values[values.length - 1], step };
}

function pad(value: number): string {
    return String(value).padStart(2, '0');
}
