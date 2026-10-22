import React from 'react';
import { Input } from '../ui/controls';
import { Select } from '../ui/overlays';
import { Text } from '../ui/surfaces';
import { INTERVAL_OPTIONS, SCHEDULE_OPTIONS, WEEKDAYS, type ScheduleForm, type ScheduleMode } from './schedule';

export const ScheduleFields: React.FC<{
    value: ScheduleForm;
    onChange: (value: ScheduleForm) => void;
}> = ({ value, onChange }) => {
    const change = (patch: Partial<ScheduleForm>) => onChange({ ...value, ...patch });
    const options = value.mode === 'custom' ? [...SCHEDULE_OPTIONS, { value: 'custom', label: '保留已有计划' }] : SCHEDULE_OPTIONS;
    const [hour = '09', minute = '00', second = '00'] = value.time.split(':');
    const setTime = (patch: Partial<{ hour: string; minute: string; second: string }>) =>
        change({ time: `${patch.hour ?? hour}:${patch.minute ?? minute}${value.mode === 'once' ? `:${patch.second ?? second}` : ''}` });
    const timeOptions = (max: number) => Array.from({ length: max + 1 }, (_, index) => ({ value: String(index).padStart(2, '0'), label: String(index).padStart(2, '0') }));
    return (
        <fieldset className="space-y-2">
            <legend className="text-xs text-text-secondary mb-1">执行计划</legend>
            <Select label="计划类型" value={value.mode} options={options} onChange={(mode) => change({ mode: mode as ScheduleMode })} />
            <div className="flex flex-wrap gap-2">
                {value.mode === 'once' && <Input type="date" aria-label="执行日期" value={value.date} onChange={(event) => change({ date: event.target.value })} className="flex-1 min-w-40" />}
                {value.mode === 'weekly' && <Select label="执行星期" value={value.weekday} options={WEEKDAYS.map((label, index) => ({ value: String(index), label }))} onChange={(weekday) => change({ weekday })} className="flex-1" />}
                {value.mode === 'monthly' && <Select label="每月执行日" value={value.day} options={Array.from({ length: 31 }, (_, index) => ({ value: String(index + 1), label: `${index + 1} 日` }))} onChange={(day) => change({ day })} className="flex-1" />}
                {['once', 'daily', 'weekly', 'monthly'].includes(value.mode) && <div className="flex items-center gap-1 flex-1 min-w-48" aria-label="执行时间"><Select label="小时" value={hour} options={timeOptions(23)} onChange={(next) => setTime({ hour: next })} className="flex-1" /><span className="text-text-caption">:</span><Select label="分钟" value={minute} options={timeOptions(59)} onChange={(next) => setTime({ minute: next })} className="flex-1" />{value.mode === 'once' && <><span className="text-text-caption">:</span><Select label="秒" value={second} options={timeOptions(59)} onChange={(next) => setTime({ second: next })} className="flex-1" /></>}</div>}
                {value.mode === 'interval' && <><Input type="number" min={1} step={1} aria-label="间隔数值" value={value.amount} onChange={(event) => change({ amount: event.target.value })} className="flex-1" /><Select label="间隔单位" value={value.unit} options={INTERVAL_OPTIONS} onChange={(unit) => change({ unit })} className="flex-1" /></>}
            </div>
            {value.mode === 'custom' && <Text size="xs" tone="secondary" className="block break-words">已有计划 {value.original} 会原样保留。选择新的计划类型后可使用日历和时间控件配置。</Text>}
            <Text size="xs" tone="tertiary" className="block">按 Gateway 所在设备的本地时区执行。{value.mode === 'monthly' ? '当月没有所选日期时跳过该月。' : ''}</Text>
        </fieldset>
    );
};
