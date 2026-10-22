import React from 'react';

import * as ipc from '../ipc';
import type { SkillItem } from '../types';

/*
 * 已安装技能清单：全应用共用一次读取。
 *
 * 指令补全要整条技能（名称加说明），正文高亮只要名字，消息气泡与输入框读的是同一份
 * 缓存，因此「什么算一条技能」在全应用只有一个口径。读失败与读到空表同义：没有候选，
 * 也不反复重试（技能目录是本机文件，坏了就是坏了，重试只会一直慢）。
 */

/** 在途或已结算的那一次读取。 */
let pending: Promise<SkillItem[]> | null = null;
/** 已结算的清单；未读完时为 null（渲染侧据此决定要不要等下再画高亮）。 */
let cached: SkillItem[] | null = null;

/** 读技能清单（首次调用发起读取，之后直接复用）。 */
export function loadSkillCatalog(): Promise<SkillItem[]> {
  if (pending === null) {
    pending = ipc.skillsList().then(
      (list) => {
        cached = list;
        return list;
      },
      () => {
        cached = [];
        return [];
      },
    );
  }
  return pending;
}

/** 订阅技能清单：未就绪时返回 null 并触发一次读取。 */
export function useSkillCatalog(): SkillItem[] | null {
  const [list, setList] = React.useState<SkillItem[] | null>(cached);
  React.useEffect(() => {
    if (list !== null) return undefined;
    let cancelled = false;
    void loadSkillCatalog().then((loaded) => {
      if (!cancelled) setList(loaded);
    });
    return () => {
      cancelled = true;
    };
  }, [list]);
  return list;
}

/** 只要名字的高亮用投影；清单未就绪时为空表，画出来就是没有高亮。 */
export function useSkillNames(): readonly string[] {
  const list = useSkillCatalog();
  return React.useMemo(() => (list === null ? [] : list.map((skill) => skill.name)), [list]);
}
