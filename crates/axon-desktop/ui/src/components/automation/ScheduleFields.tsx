import React from 'react';
import { AlertCircleIcon } from '../../icons';
import { Button, Input } from '../ui/controls';
import { Select } from '../ui/overlays';
import { Banner, FormField, Text } from '../ui/surfaces';
import {
    INTERVAL_OPTIONS,
    SCHEDULE_OPTIONS,
    WEEKDAYS,
    encodeSchedule,
    scheduleError,
    scheduleText,
    type ScheduleForm,
    type ScheduleMode,
} from './schedule';

const HOURS = Array.from({ length: 24 }, (_, index) => ({
    value: String(index).padStart(2, '0'),
    label: `${index} 点`,
}));
const DAYS = Array.from({ length: 31 }, (_, index) => ({ value: String(index + 1), label: `${index + 1} 日` }));
const SPAN_CADENCE = [
    { value: '1|hour', label: '每小时' },
    { value: '2|hour', label: '每 2 小时' },
    { value: '3|hour', label: '每 3 小时' },
    { value: '1|minute', label: '每分钟' },
    { value: '5|minute', label: '每 5 分钟' },
    { value: '10|minute', label: '每 10 分钟' },
    { value: '15|minute', label: '每 15 分钟' },
    { value: '30|minute', label: '每 30 分钟' },
];

export const ScheduleFields: React.FC<{
    value: ScheduleForm;
    onChange: (value: ScheduleForm) => void;
}> = ({ value, onChange }) => {
    const change = (patch: Partial<ScheduleForm>) => onChange({ ...value, ...patch });
    const options = value.original ? [...SCHEDULE_OPTIONS, { value: 'custom', label: '保留已有计划' }] : SCHEDULE_OPTIONS;
    const [hour = '09', minute = '00', second = '00'] = value.time.split(':');
    const setTime = (patch: Partial<{ hour: string; minute: string; second: string }>) =>
        change({ time: `${patch.hour ?? hour}:${patch.minute ?? minute}${value.mode === 'once' ? `:${patch.second ?? second}` : ''}` });
    const timeOptions = (max: number) => Array.from({ length: max + 1 }, (_, index) => ({ value: String(index).padStart(2, '0'), label: String(index).padStart(2, '0') }));

    const expression = encodeSchedule(value);
    const preview = scheduleText(expression);
    const error = scheduleError(value);
    const cadence = `${Number(value.spanEvery) || 1}|${value.spanUnit}`;
    const cadenceLabel = `${cadence.startsWith('1|') ? '' : `每 ${cadence.split('|')[0]} `}${value.spanUnit === 'minute' ? '分钟' : '小时'}`;
    const cadenceOptions = SPAN_CADENCE.some((option) => option.value === cadence)
        ? SPAN_CADENCE
        : [...SPAN_CADENCE, { value: cadence, label: cadenceLabel }];
    const supportsSpan = value.mode !== 'interval' && value.mode !== 'once' && value.mode !== 'custom';

    const toggleWeekday = (day: string) => {
        const weekdays = value.weekdays.includes(day)
            ? value.weekdays.filter((item) => item !== day)
            : [...value.weekdays, day];
        change({ weekdays });
    };

    return (
        <fieldset className="space-y-3">
            <legend className="mb-1 text-xs text-text-secondary">执行计划</legend>
            <FormField label="计划类型">
                <Select label="计划类型" value={value.mode} options={options} onChange={(mode) => {
                    const next = mode as ScheduleMode;
                    const keepsSpan = next !== 'interval' && next !== 'once' && next !== 'custom';
                    change({ mode: next, ...(keepsSpan ? {} : { span: false }) });
                }} />
            </FormField>

            {value.mode === 'custom' ? (
                <div className="rounded-sm bg-surface-sunken px-3 py-2">
                    <Text size="sm" strong className="block">保留已有计划</Text>
                    <Text size="xs" tone="tertiary" className="mt-1 block break-all">{value.original}</Text>
                    <Text size="xs" tone="secondary" className="mt-1 block">该计划由命令行或更复杂的表达式创建，选择其他计划类型后可改用控件配置。</Text>
                </div>
            ) : (
                <>
                    <div className="flex flex-wrap items-center gap-2">
                        {value.mode === 'once' && (
                            <Input type="date" aria-label="执行日期" value={value.date} onChange={(event) => change({ date: event.target.value })} className="min-w-40 flex-1" />
                        )}
                        {value.mode === 'monthly' && (
                            <Select label="每月执行日" value={value.day} options={DAYS} onChange={(day) => change({ day })} className="flex-1" />
                        )}
                        {value.mode === 'interval' && (
                            <>
                                <Input type="number" min={1} step={1} aria-label="间隔数值" value={value.amount} onChange={(event) => change({ amount: event.target.value })} className="flex-1" />
                                <Select label="间隔单位" value={value.unit} options={INTERVAL_OPTIONS} onChange={(unit) => change({ unit })} className="flex-1" />
                            </>
                        )}
                        {value.mode !== 'interval' && !value.span && (
                            <div role="group" aria-label="执行时间" className="flex min-w-48 flex-1 items-center gap-1">
                                <Select label="小时" value={hour} options={timeOptions(23)} onChange={(next) => setTime({ hour: next })} className="flex-1" />
                                <span className="text-text-caption">:</span>
                                <Select label="分钟" value={minute} options={timeOptions(59)} onChange={(next) => setTime({ minute: next })} className="flex-1" />
                                {value.mode === 'once' && (
                                    <>
                                        <span className="text-text-caption">:</span>
                                        <Select label="秒" value={second} options={timeOptions(59)} onChange={(next) => setTime({ second: next })} className="flex-1" />
                                    </>
                                )}
                            </div>
                        )}
                    </div>

                    {value.mode === 'weekly' && (
                        <div className="space-y-1.5">
                            <div role="group" aria-label="执行星期" className="flex flex-wrap gap-1.5">
                                {WEEKDAYS.map((label, index) => {
                                    const day = String(index);
                                    const active = value.weekdays.includes(day);
                                    return (
                                        <Button
                                            key={day}
                                            size="sm"
                                            shape="pill"
                                            variant={active ? 'primary' : 'outline'}
                                            aria-pressed={active}
                                            onClick={() => toggleWeekday(day)}
                                        >
                                            {label}
                                        </Button>
                                    );
                                })}
                            </div>
                            <Text size="xs" tone="tertiary" className="block">可多选；全部不选时按每天执行。</Text>
                        </div>
                    )}

                    {supportsSpan && (
                        <div className="space-y-2">
                            <Button
                                size="sm"
                                shape="pill"
                                variant={value.span ? 'primary' : 'outline'}
                                aria-pressed={value.span}
                                onClick={() => change({ span: !value.span })}
                            >
                                指定时段
                            </Button>
                            {value.span && (
                                <div className="flex flex-wrap items-end gap-2">
                                    <FormField label="起始" className="flex-1">
                                        <Select label="起始小时" value={value.spanStart} options={HOURS} onChange={(spanStart) => change({ spanStart })} className="w-full" />
                                    </FormField>
                                    <FormField label="结束" className="flex-1">
                                        <Select label="结束小时" value={value.spanEnd} options={HOURS} onChange={(spanEnd) => change({ spanEnd })} className="w-full" />
                                    </FormField>
                                    <FormField label="频率" className="flex-1">
                                        <Select label="时段频率" value={cadence} options={cadenceOptions} onChange={(next) => {
                                            const [every, unit] = next.split('|');
                                            change({ spanEvery: every, spanUnit: unit as 'hour' | 'minute' });
                                        }} className="w-full" />
                                    </FormField>
                                </div>
                            )}
                        </div>
                    )}

                    <div className="rounded-sm bg-surface-sunken px-3 py-2">
                        <Text size="sm" strong className="block">{preview}</Text>
                        <Text size="xs" tone="tertiary" className="mt-1 block break-all">{expression}</Text>
                    </div>

                    {error && <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={error} />}
                </>
            )}

            <Text size="xs" tone="tertiary" className="block">
                按 Gateway 所在设备的本地时区执行。workday、holiday、weekend 三个前缀读取已同步的中国年历；未同步的年份里，工作日按周一到周五执行，法定节假日不会触发。
            </Text>
        </fieldset>
    );
};
