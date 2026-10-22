import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { AlertCircleIcon } from '../../icons';
import type { SkillItem } from '../../types';
import * as ipc from '../../ipc';
import { Banner, Chip } from '../ui/surfaces';
import { CardDetailDialog, CardList, EntityCard } from '../ui/cards';

/*
 * 技能管理：以卡片列表呈现已安装技能，支持搜索与查看详情。
 * 技能本体是磁盘上的目录与 SKILL.md，界面只做只读呈现；
 * 打开技能目录由设置分区标题行的入口统一承担，卡片内不重复放置按钮。
 */

export const SkillsSettings: React.FC = () => {
  const [skills, setSkills] = useState<SkillItem[]>([]);
  const [query, setQuery] = useState('');
  const [detail, setDetail] = useState<SkillItem | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    try {
      setSkills(await ipc.skillsList());
      setError(null);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '读取技能列表失败');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const visible = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (keyword.length === 0) return skills;
    return skills.filter(
      (skill) =>
        skill.name.toLowerCase().includes(keyword) ||
        skill.id.toLowerCase().includes(keyword) ||
        skill.description.toLowerCase().includes(keyword),
    );
  }, [skills, query]);

  return (
    <div className="space-y-3">
      {error !== null && <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={error} />}

      <CardList
        query={query}
        onQueryChange={setQuery}
        placeholder="搜索技能名称或说明"
        summary={`共 ${visible.length} 项技能`}
        loading={loading}
        emptyTitle={query.trim().length > 0 ? '没有匹配的技能' : '技能目录中没有可用技能'}
        emptyDescription={
          query.trim().length > 0 ? '换一个关键词再试' : '把技能目录放进 Axon 技能路径后重新打开设置'
        }
      >
        {visible.map((skill) => (
          <EntityCard
            key={skill.id}
            title={skill.name}
            badges={skill.name === skill.id ? undefined : <Chip>{skill.id}</Chip>}
            description={skill.description.length > 0 ? skill.description : '该技能未填写说明。'}
            locator={skill.path}
            meta={skill.entry_path.length > 0 ? '含 SKILL.md' : '缺少 SKILL.md'}
            onOpen={() => setDetail(skill)}
            onInspect={() => setDetail(skill)}
          />
        ))}
      </CardList>

      <CardDetailDialog
        open={detail !== null}
        onClose={() => setDetail(null)}
        title={detail?.name ?? '技能详情'}
        fields={[
          { label: '技能标识', value: detail?.id ?? '', mono: true },
          { label: '技能说明', value: detail?.description ?? '' },
          { label: '所在目录', value: detail?.path ?? '', mono: true },
          { label: '描述文件', value: detail?.entry_path ?? '', mono: true },
        ]}
      />
    </div>
  );
};
