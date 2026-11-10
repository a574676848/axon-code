import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { AlertCircleIcon, TrashIcon } from '../../icons';
import type { SkillItem } from '../../types';
import * as ipc from '../../ipc';
import { reloadSkillCatalog } from '../../conversation/skillCatalog';
import { Dialog } from '../ui/overlays';
import { Banner, Chip, Text } from '../ui/surfaces';
import { CardDetailDialog, CardList, EntityCard } from '../ui/cards';

/*
 * 技能管理：以卡片列表呈现已安装技能，支持搜索、查看详情与移除。
 *
 * 技能本体是磁盘上的目录与 SKILL.md，移除就是删掉那个目录，因此详情里的移除不直接
 * 动手，只摊开确认框；删完再重读全应用共用的技能清单，指标条计数、斜杠候选与正文
 * 高亮不必等到重启。打开技能目录由设置分区标题行的入口统一承担，卡片内不重复放置按钮。
 */

export const SkillsSettings: React.FC = () => {
  const [skills, setSkills] = useState<SkillItem[]>([]);
  const [query, setQuery] = useState('');
  const [detail, setDetail] = useState<SkillItem | null>(null);
  const [confirming, setConfirming] = useState<SkillItem | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [removeError, setRemoveError] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);
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

  const remove = async (skill: SkillItem) => {
    setRemoving(true);
    setRemoveError(null);
    try {
      await ipc.skillsRemove(skill.id);
      setConfirming(null);
      setDetail(null);
      await load();
      void reloadSkillCatalog();
    } catch (failure) {
      // 失败原因留在确认框里：它盖在详情之上，报在这一层用户才看得到。
      setRemoveError(failure instanceof Error ? failure.message : '移除技能失败');
    } finally {
      setRemoving(false);
    }
  };

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
        secondary={{
          label: '移除',
          icon: <TrashIcon size={13} />,
          danger: true,
          onClick: () => {
            setRemoveError(null);
            setConfirming(detail);
          },
        }}
      />

      <Dialog
        open={confirming !== null}
        onClose={() => {
          if (!removing) setConfirming(null);
        }}
        title={`移除技能 ${confirming?.name ?? ''}`}
        size="sm"
        primary={{
          label: '确认移除',
          busyLabel: '移除中',
          icon: <TrashIcon size={13} />,
          busy: removing,
          onClick: () => {
            if (confirming !== null) void remove(confirming);
          },
        }}
        secondary={{
          label: '取消',
          disabled: removing,
          onClick: () => setConfirming(null),
        }}
      >
        <Text size="row" className="block leading-relaxed">
          移除会删除整个技能目录，Axon 与技能清单都不再加载它，删除后无法从这里恢复。
        </Text>
        <Text size="code" mono tone="tertiary" className="block break-all">
          {confirming?.path ?? ''}
        </Text>
        {removeError !== null && (
          <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={removeError} />
        )}
      </Dialog>
    </div>
  );
};
