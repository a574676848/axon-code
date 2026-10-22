import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertCircleIcon,
  BrainIcon,
  ChartIcon,
  ChecklistIcon,
  DatabaseIcon,
  GaugeIcon,
  GlobeIcon,
  PriceTagIcon,
  PulseIcon,
  SettingsIcon,
  SkillIcon,
  SlidersIcon,
  ToolIcon,
  UserIcon,
} from '../icons';
import type { ConfigReadResult, SettingsSection } from '../types';
import { isEmbeddingModel } from '../modelCatalog';
import * as ipc from '../ipc';
import { Modal } from './ui/overlays';
import { Button } from './ui/controls';
import { Banner, EmptyState, HairlineStack, LoadingState, Text } from './ui/surfaces';
import { ConfigFieldRow } from './ConfigFieldRow';
import { ModelsSettings } from './settings/ProviderProfileCard';
import { McpSettings } from './settings/McpSettings';
import { SkillsSettings } from './settings/SkillsSettings';
import { DocumentEditor } from './settings/DocumentEditor';
import { SystemInfoSettings } from './settings/SystemInfoSettings';
import { SystemSettings } from './settings/SystemSettings';
import { UsageStatistics } from './settings/UsageStatistics';
import { PricingSettings } from './settings/PricingSettings';
import { HeaderDocumentAction } from './settings/HeaderDocumentAction';
import { SettingsHeaderProvider } from './settings/SettingsHeaderSlot';

/*
 * 设置弹窗：左侧二级分区，右侧正文。
 * 没有校验与保存按钮：每个配置项一经改动就立即写入权威配置，
 * 失败时就地给出错误条，并回滚显示。
 *
 * 所有分区共用一套面板尺寸与一条底部状态行：
 * 切换分区时窗口不跳动，短分区的空区也有明确的下沿。
 */

interface SectionSpec {
  id: SettingsSection;
  label: string;
  /** 分区说明：一句话讲清这一屏管什么。 */
  description: string;
  /** 分区底部的状态行文案。 */
  footer: string;
  icon: React.FC<{ size?: number; className?: string }>;
  prefixes: string[];
}

const SECTIONS: SectionSpec[] = [
  {
    id: 'general',
    label: '通用设置',
    description: '对话模型、推理强度与子代理并发等运行偏好',
    footer: '每一项改动即时写入 config.toml',
    icon: SlidersIcon,
    prefixes: ['agent.'],
  },
  {
    id: 'models',
    label: '模型',
    description: '供应商连接参数与它名下的模型清单',
    footer: '每一项改动即时写入 config.toml',
    icon: DatabaseIcon,
    prefixes: [],
  },
  // 定价紧跟模型：价格的单位就是「每个模型每百万 Token 多少钱」。
  {
    id: 'pricing',
    label: '模型定价',
    description: '按模型匹配式配置单价，用量看板据此计算成本',
    footer: '价格规则保存在本地定价库，改动即时生效',
    icon: PriceTagIcon,
    prefixes: [],
  },
  {
    id: 'balancer',
    label: '负载均衡',
    description: '通道熔断、亲和时长与遥测留存',
    footer: '每一项改动即时写入 config.toml',
    icon: GaugeIcon,
    prefixes: ['channel_balancer.'],
  },
  {
    id: 'remote',
    label: '远控',
    description: '网关监听地址与飞书长连接远控',
    footer: '每一项改动即时写入 config.toml',
    icon: PulseIcon,
    prefixes: ['remote.'],
  },
  {
    id: 'memory',
    label: '记忆',
    description: '长期记忆的向量检索与嵌入模型',
    footer: '每一项改动即时写入 config.toml',
    icon: BrainIcon,
    prefixes: ['memory.', 'skills.'],
  },
  {
    id: 'mcp',
    label: 'MCP',
    description: '本地进程与远端端点两类服务器',
    footer: '每一项改动即时写入 .mcp.json',
    icon: ToolIcon,
    prefixes: [],
  },
  {
    id: 'skills',
    label: '技能',
    description: '技能目录中的只读清单',
    footer: '技能来自技能目录，此处为只读清单',
    icon: SkillIcon,
    prefixes: [],
  },
  {
    id: 'soul',
    label: '人设',
    description: '长期人设与价值观，作用于所有会话',
    footer: '修改在点保存后原子写入源文件',
    icon: UserIcon,
    prefixes: [],
  },
  {
    id: 'agents',
    label: '行为',
    description: '全局行为约束，作用于所有会话',
    footer: '修改在点保存后原子写入源文件',
    icon: ChecklistIcon,
    prefixes: [],
  },
  {
    id: 'usage',
    label: '用量统计',
    description: '请求、Token、延迟与成本的观测',
    footer: '',
    icon: ChartIcon,
    prefixes: [],
  },
  {
    id: 'system',
    label: '系统设置',
    description: '桌面外壳偏好与运行状态',
    footer: '桌面偏好保存在本机，运行状态来自实时探测',
    icon: SettingsIcon,
    prefixes: [],
  },
  // 关于排在系统设置之后：它只读、几乎不常改，放在配置类分区的末尾。
  {
    id: 'about',
    label: '关于',
    description: '产品身份、版本与发布渠道',
    footer: '版本与渠道信息来自构建期元数据',
    icon: GlobeIcon,
    prefixes: [],
  },
];

/** 把任意配置值转成界面可编辑的字符串。 */
function toEditableString(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  if (Array.isArray(value)) return value.map((item) => String(item)).join(',');
  return JSON.stringify(value);
}

/** 逐项配置的分区：这些分区的正文来自 config.specs。 */
const PREFIX_SECTIONS = new Set<SettingsSection>(['general', 'balancer', 'remote', 'memory']);

interface BasicSettingsPanelProps {
  /** 是否打开。 */
  open: boolean;
  /** 关闭。 */
  onClose: () => void;
  /** 打开时定位到的分区。 */
  initialSection?: SettingsSection;
}

export const BasicSettingsPanel: React.FC<BasicSettingsPanelProps> = ({
  open,
  onClose,
  initialSection = 'general',
}) => {
  const [config, setConfig] = useState<ConfigReadResult | null>(null);
  const [section, setSection] = useState<SettingsSection>('general');
  const [loading, setLoading] = useState(false);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [actionSlot, setActionSlot] = useState<HTMLElement | null>(null);
  const [statusSlot, setStatusSlot] = useState<HTMLElement | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setConfig(await ipc.readConfig());
      setError(null);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '读取配置失败');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (open) void load();
  }, [open, load]);

  // 打开时按调用方要求定位分区，从状态入口进来会直接落在系统信息。
  useEffect(() => {
    if (open) setSection(initialSection);
  }, [open, initialSection]);

  // 切分区回到顶部：从长分区切到短分区时，沿用旧滚动位置会先看到一片空白。
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
  }, [section]);

  const spec = SECTIONS.find((item) => item.id === section) ?? SECTIONS[0];

  /** 当前分区对应的配置键。 */
  const fields = useMemo(() => {
    if (config === null) return [];
    return config.specs.filter((item) =>
      spec.prefixes.some((prefix) => item.key.startsWith(prefix)),
    );
  }, [config, spec]);

  /** 支持向量检索的模型；向量模型键只能从中选择。 */
  const embeddingModelNames = useMemo(
    () =>
      (config?.models ?? [])
        .filter(isEmbeddingModel)
        .map((model) => model.name),
    [config],
  );
  const modelNames = useMemo(
    () =>
      (config?.models ?? [])
        .filter((model) => !isEmbeddingModel(model))
        .map((model) => model.name),
    [config],
  );

  /** 单项立即生效：写入成功后重读，保证界面永远显示磁盘真值。 */
  const applyValue = async (key: string, value: string) => {
    setBusyKey(key);
    setError(null);
    try {
      await ipc.applyConfigValues({ [key]: value });
      await load();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '写入配置失败');
    } finally {
      setBusyKey(null);
    }
  };

  /*
   * 用量看板与模型定价比其余分区宽：一张时序图或一张价格表挤在 720px 里读不出信息，
   * 因此这两个分区铺满正文列并铺下沉底板；表单分区仍收在 720px 的内容列里。
   * 面板本身只有一套尺寸，切换分区不会让窗口左右跳。
   */
  const dashboard = section === 'usage' || section === 'pricing';
  // 关于页没有表单列：整块内容在正文区里居中，而不是缩在左侧 720px 里。
  const wideContent = dashboard || section === 'about';
  const needsConfig = PREFIX_SECTIONS.has(section) || section === 'models';

  const configBody = needsConfig && config === null;

  const prefixedFields = (
    <HairlineStack>
      {fields.map((item) => (
        <ConfigFieldRow
          key={item.key}
          spec={item}
          value={toEditableString(config?.flat_values[item.key])}
          modelNames={modelNames}
          embeddingModelNames={embeddingModelNames}
          busy={busyKey !== null}
          onChange={(next) => void applyValue(item.key, next)}
        />
      ))}
    </HairlineStack>
  );

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="设置"
      variant="floating"
      panelClassName="w-full max-w-[960px] h-[calc(100vh-64px)] max-h-[720px]"
    >
      {/* 二级分区导航 */}
      <nav
        aria-label="设置分区"
        className="w-[184px] shrink-0 p-2 space-y-0.5 bg-surface-sunken overflow-y-auto scroll-visible hairline-r border-border-hairline"
      >
        {SECTIONS.map((item) => {
          const Icon = item.icon;
          const active = item.id === section;
          return (
            <button
              key={item.id}
              type="button"
              aria-current={active ? 'page' : undefined}
              onClick={() => setSection(item.id)}
              className={`w-full flex items-center gap-2.5 h-8 px-2.5 rounded-sm text-left transition-colors duration-100 ${
                active
                  ? 'bg-active text-text-primary font-medium'
                  : 'text-text-secondary hover:text-text-primary hover:bg-hover'
              }`}
            >
              <Icon size={15} className="shrink-0" />
              <Text size="row" truncate className="min-w-0">
                {item.label}
              </Text>
            </button>
          );
        })}
      </nav>

      {/* 正文 */}
      <div className="flex-1 min-w-0 min-h-0 flex flex-col bg-surface">
        <div ref={scrollRef} className="flex-1 min-h-0 overflow-y-auto scroll-visible">
          <div className={dashboard ? 'min-h-full bg-surface-sunken' : 'min-h-full flex flex-col'}>
            <div
              className={`${wideContent ? 'max-w-none' : 'max-w-[720px]'} flex-1 flex flex-col pl-5 pr-6 pt-3 pb-5`}
            >
              {/* 分区身份：标题 16/24 500，说明 12/16 secondary，动作与内容右轴同轴 */}
              <div className="flex items-start justify-between gap-4">
                <Text size="base" strong truncate className="min-w-0">
                  {spec.label}
                </Text>
                <div className="shrink-0 flex items-center gap-2">
                  <HeaderDocumentAction section={section} />
                  <div ref={setActionSlot} className="flex items-center gap-2" />
                </div>
              </div>
              <div className="mt-0.5 flex items-start justify-between gap-3">
                <Text tone="secondary" size="xs" className="min-w-0">
                  {spec.description}
                </Text>
                <div ref={setStatusSlot} className="shrink-0 flex items-center gap-2" />
              </div>

              {error !== null && (
                <div className="mt-3">
                  <Banner
                    tone="danger"
                    icon={<AlertCircleIcon size={13} />}
                    title={`配置操作失败：${error}`}
                    action={
                      <Button variant="ghost" size="sm" onClick={() => setError(null)}>
                        知道了
                      </Button>
                    }
                  />
                </div>
              )}

              <div className="mt-4 flex-1 flex flex-col">
                <SettingsHeaderProvider actionSlot={actionSlot} statusSlot={statusSlot}>
                  {configBody ? (
                    loading ? (
                      <LoadingState label="正在读取配置" />
                    ) : (
                      <EmptyState
                        title="无法读取配置"
                        description={error ?? '配置暂时不可用，请稍后重试'}
                      />
                    )
                  ) : (
                    <>
                      {section === 'models' && config !== null && (
                        <ModelsSettings
                          providers={config.providers}
                          models={config.models}
                          protocols={config.provider_protocols}
                          wireTransports={config.provider_wire_transports}
                          onChanged={() => void load()}
                        />
                      )}

                      {section === 'mcp' && <McpSettings />}

                      {section === 'skills' && <SkillsSettings />}

                      {section === 'soul' && (
                        <DocumentEditor
                          documentId="soul"
                          placeholder="写下 Axon 的人设与长期偏好"
                        />
                      )}

                      {section === 'agents' && (
                        <DocumentEditor
                          documentId="agents"
                          placeholder="写下必须遵守的行为约束与工程规范"
                        />
                      )}

                      {section === 'system' && <SystemSettings />}

                      {section === 'usage' && <UsageStatistics />}

                      {section === 'pricing' && <PricingSettings />}

                      {section === 'about' && <SystemInfoSettings />}

                      {PREFIX_SECTIONS.has(section) && (
                        <>
                          {fields.length === 0 ? (
                            <EmptyState
                              title="该分区下没有可配置项"
                              description="配置文件里没有与这个分区匹配的键"
                            />
                          ) : (
                            prefixedFields
                          )}
                        </>
                      )}
                    </>
                  )}
                </SettingsHeaderProvider>
              </div>
            </div>
          </div>
        </div>

        {/* 底部状态行：与内容无关，永远停在面板底部；没有状态文案时整条不出现。 */}
        {spec.footer.length > 0 && (
          <footer className="shrink-0 h-8 px-5 flex items-center hairline-t border-border-hairline">
            <Text tone="tertiary" size="xs" className="min-w-0 truncate">
              {spec.footer}
            </Text>
          </footer>
        )}
      </div>
    </Modal>
  );
};
