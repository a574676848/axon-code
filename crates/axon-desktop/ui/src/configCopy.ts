import type { ConfigKeySpec } from './types';

/*
 * 配置项的中文产品化文案。
 *
 * 界面只呈现这里的文案，不直接把配置键或带英文标识的原始描述抛给用户：
 * 标签是一句话能读懂的中文名称，说明是一行补充，不出现 name / id / catalog
 * 这类实现词汇。新增配置键时在此登记，缺失时回落到清洗后的描述。
 */

/** 单个配置项的文案。 */
interface ConfigCopy {
  /** 中文标签。 */
  label: string;
  /** 一行补充说明；不需要补充时省略。 */
  detail?: string;
}

/** 按配置键登记的文案。 */
const CONFIG_COPY: Record<string, ConfigCopy> = {
  // 通用设置
  'agent.default_model': { label: '主模型', detail: '默认使用的对话模型' },
  'agent.fast_model': { label: '快速模型', detail: '处理轻量请求的模型' },
  'agent.reasoning_effort': { label: '推理强度', detail: '主模型的思考深度' },
  'agent.log_level': { label: '日志级别', detail: '记录日志的详细程度' },
  'agent.max_subagent_threads': {
    label: '子代理线程上限',
    detail: '单个会话可并发的子代理数量',
  },
  'agent.max_subagent_active_turns': {
    label: '并发子任务上限',
    detail: '单个会话同时运行的子任务数量',
  },
  'agent.max_subagent_depth': { label: '子代理嵌套深度', detail: '子代理可继续派生的层数' },
  'agent.idle_compaction_debounce_ms': {
    label: '闲置压缩延迟',
    detail: '空闲多久后压缩上下文，单位毫秒',
  },

  // 负载均衡
  'channel_balancer.failure_threshold': {
    label: '熔断失败次数',
    detail: '连续失败多少次后暂停该通道',
  },
  'channel_balancer.cooldown_ms': { label: '熔断冷却时长', detail: '暂停多久后恢复，单位毫秒' },
  'channel_balancer.affinity_ttl_ms': {
    label: '通道亲和时长',
    detail: '同一任务固定走同一通道的时长，单位毫秒',
  },
  'channel_balancer.telemetry_persistence_enabled': {
    label: '保存渠道遥测',
    detail: '是否把渠道指标落盘留存',
  },

  // 记忆
  'memory.embedding.enabled': { label: '启用记忆检索', detail: '是否开启长期记忆的向量检索' },
  'memory.embedding.model': { label: '向量模型', detail: '用于记忆检索的嵌入模型' },

  // 远控
  'remote.gateway.api_token_env': {
    label: '网关密钥变量名',
    detail: '存放网关访问密钥的环境变量',
  },
  'remote.gateway.http_bind': { label: 'HTTP 监听地址', detail: '接收 Webhook 的地址' },
  'remote.gateway.websocket_bind': {
    label: 'WebSocket 监听地址',
    detail: '接收长连接的地址',
  },
  'remote.mode': { label: '远控模式', detail: '常规模式够用，高频事件流再切换' },
  'remote.feishu_websocket.enabled': { label: '启用飞书远控', detail: '是否开启飞书长连接远控' },
  'remote.feishu_websocket.app_id': { label: '飞书应用 ID', detail: '飞书开放平台的应用标识' },
  'remote.feishu_websocket.app_secret_env': {
    label: '飞书密钥变量名',
    detail: '存放应用密钥的环境变量',
  },
  'remote.feishu_websocket.api_base_url': {
    label: '飞书接口地址',
    detail: '留空使用官方默认地址',
  },
  'remote.feishu_websocket.websocket_url': {
    label: '飞书长连接地址',
    detail: '留空使用官方默认地址',
  },
  'remote.feishu_websocket.enable_notification_sending': {
    label: '发送远控通知',
    detail: '是否允许主动推送通知',
  },
  'remote.feishu_websocket.notification_chat_id': {
    label: '消息通知 chat_id',
    detail: '后台完成通知的接收会话；留空不通知飞书',
  },
  'remote.feishu_websocket.allowed_sender_open_ids': {
    label: '允许的发送者',
    detail: '仅这些用户可以远控',
  },
  'remote.feishu_websocket.blocked_sender_open_ids': {
    label: '禁止的发送者',
    detail: '这些用户一律拒绝',
  },
  'remote.feishu_websocket.require_mention': {
    label: '需要 @ 机器人',
    detail: '群聊中必须 @ 才响应',
  },
};

/** 判断文本里是否含中文。 */
function hasChinese(text: string): boolean {
  return /[\u4e00-\u9fa5]/.test(text);
}

/** 去掉描述里的实现词汇，只保留面向用户的一句中文。 */
function cleanDescription(description: string): string {
  return description
    .replace(/`[^`]*`/g, '')
    .split(/[，,。；;：:（(]/)
    .map((piece) => piece.trim())
    .filter((piece) => piece.length > 0 && hasChinese(piece))
    .join('，');
}

/** 取配置项的中文标签。 */
export function configLabel(spec: ConfigKeySpec): string {
  const copy = CONFIG_COPY[spec.key];
  if (copy !== undefined) return copy.label;
  const cleaned = cleanDescription(spec.description);
  return cleaned.length > 0 ? cleaned : '未命名配置项';
}

/** 取配置项的一行说明；没有可读说明时返回空串。 */
export function configDetail(spec: ConfigKeySpec): string {
  const copy = CONFIG_COPY[spec.key];
  if (copy !== undefined) return copy.detail ?? '';
  return '';
}
