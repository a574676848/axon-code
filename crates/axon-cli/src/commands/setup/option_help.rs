use axon_config::ConfigValueKind;

pub(super) fn key_detail(key: &str) -> Option<&'static str> {
    agent_key_detail(key)
        .or_else(|| channel_balancer_key_detail(key))
        .or_else(|| memory_key_detail(key))
        .or_else(|| remote_key_detail(key))
        .or_else(|| tui_key_detail(key))
}

fn agent_key_detail(key: &str) -> Option<&'static str> {
    match key {
        "agent.default_model" => Some("主 Agent 固定使用的模型别名或模型 ID。"),
        "agent.fast_model" => Some("delegate_to_fast 使用的快速模型别名或模型 ID。"),
        "agent.reasoning_effort" => {
            Some("主 Agent default_model 的 reasoning effort；委托工具不读取该值。")
        }
        "agent.max_subagent_threads" => Some("每个 run session 允许创建的最大子代理线程数。"),
        "agent.max_subagent_active_turns" => {
            Some("每个 run session 允许同时运行的子代理 turn 数，不能超过线程数。")
        }
        "agent.max_subagent_depth" => Some("子代理允许继续派生时的最大嵌套深度，root 深度为 0。"),
        "agent.idle_compaction_debounce_ms" => {
            Some("会话进入闲暇压缩前等待的防抖时长，单位为毫秒。")
        }
        _ => None,
    }
}

fn channel_balancer_key_detail(key: &str) -> Option<&'static str> {
    match key {
        "channel_balancer.failure_threshold" => Some("单个通道连续失败达到此次数后进入熔断。"),
        "channel_balancer.cooldown_ms" => Some("熔断后暂停尝试该通道的冷却时长，单位为毫秒。"),
        "channel_balancer.affinity_ttl_ms" => Some("同一任务保持通道亲和关系的时长，单位为毫秒。"),
        "channel_balancer.telemetry_persistence_enabled" => {
            Some("开启后将渠道性能与缓存遥测追加保存到 data/channel-metrics。")
        }
        _ => None,
    }
}

fn memory_key_detail(key: &str) -> Option<&'static str> {
    match key {
        "memory.embedding.enabled" => Some("启用长期记忆的 embedding 写入与语义检索。"),
        "memory.embedding.model" => {
            Some("长期记忆使用的 embedding 模型，必须引用已配置 embedding 模型。")
        }
        _ => None,
    }
}

fn remote_key_detail(key: &str) -> Option<&'static str> {
    match key {
        "remote.gateway.api_token_env" => Some(
            "统一 Gateway API token 的环境变量名；只保存变量名，token 原文写入用户级 env 文件。",
        ),
        "remote.gateway.http_bind" => {
            Some("HTTP Webhook 监听地址，例如 127.0.0.1:3143 或 0.0.0.0:3143。")
        }
        "remote.gateway.websocket_bind" => {
            Some("通用 WebSocket 监听地址，例如 127.0.0.1:3144 或 0.0.0.0:3144。")
        }
        "remote.mode" => Some("远控运行模式：standard 或 high-throughput。"),
        "remote.feishu_websocket.enabled" => {
            Some("启用 Feishu WebSocket 长连接 adapter；远控 Gateway 目标态始终启用。")
        }
        "remote.feishu_websocket.app_id" => {
            Some("飞书开放平台应用的 App ID。不要填写 App Secret。")
        }
        "remote.feishu_websocket.app_secret_env" => {
            Some("保存飞书 App Secret 的环境变量名；setup 只保存变量名，不保存密钥明文。")
        }
        "remote.feishu_websocket.api_base_url" => {
            Some("飞书 OpenAPI 基础地址。留空使用官方默认地址，私有化或代理环境才需要填写。")
        }
        "remote.feishu_websocket.websocket_url" => {
            Some("飞书长连接地址。留空使用官方默认地址，私有化或代理环境才需要填写。")
        }
        "remote.feishu_websocket.enable_notification_sending" => {
            Some("允许 adapter 主动发送远控通知。关闭时只接收和处理输入。")
        }
        "remote.feishu_websocket.notification_chat_id" => {
            Some("后台完成通知接收会话的 chat_id。留空不通知飞书；不会从历史聊天自动选择收件人。")
        }
        "remote.feishu_websocket.allowed_sender_open_ids" => {
            Some("允许访问远控的飞书 open_id 列表。留空表示不额外限制 allowlist。")
        }
        "remote.feishu_websocket.blocked_sender_open_ids" => {
            Some("禁止访问远控的飞书 open_id 列表；命中时即使在 allowlist 中也会被拒绝。")
        }
        "remote.feishu_websocket.require_mention" => {
            Some("群聊中要求显式 @ 机器人后才处理消息，降低误触发概率。")
        }
        _ => None,
    }
}

fn tui_key_detail(_key: &str) -> Option<&'static str> {
    None
}

pub(super) fn input_hint(key: &str, kind: ConfigValueKind, has_options: bool) -> &'static str {
    if has_options {
        return "输入序号或直接输入可选值；回车保留当前值。";
    }
    match key {
        "agent.default_model" | "agent.fast_model" => {
            "输入模型别名或模型 ID；通常使用 setup 中配置的模型别名。"
        }
        "remote.feishu_websocket.api_base_url" | "remote.feishu_websocket.websocket_url" => {
            "输入完整 URL；留空表示使用官方默认地址。"
        }
        "remote.feishu_websocket.app_secret_env" => "输入环境变量名，不要输入真实密钥。",
        "remote.feishu_websocket.notification_chat_id" => {
            "输入通知会话的 chat_id；输入“留空”关闭飞书后台通知。"
        }
        _ => match kind {
            ConfigValueKind::Bool => "输入 true 或 false；也可输入序号；回车保留当前值。",
            ConfigValueKind::Unsigned => "输入非负整数；请按说明中的单位填写；回车保留当前值。",
            ConfigValueKind::String => "输入文本值；需要清空时输入“留空”；回车保留当前值。",
            ConfigValueKind::StringList => "输入列表项，用英文逗号分隔；需要清空时输入“留空”。",
        },
    }
}

pub(super) fn option_detail(context: &str, value: &str) -> Option<&'static str> {
    match value {
        "跳过" => return Some("保留当前模块配置，本轮不修改"),
        "重新配置" => return Some("重新填写该模块配置，并覆盖对应字段"),
        "保留当前值" => return Some("不修改当前字段"),
        "留空" => return Some("清空可选字段或跳过可选写入"),
        _ => {}
    }
    match (context, value) {
        (_, "true") => Some("启用"),
        (_, "false") => Some("关闭"),
        ("Provider 协议", "openai") => Some("OpenAI 协议（默认 Responses API）"),
        ("Provider 协议", "openai-chatcompletions") => Some("OpenAI Chat Completions 协议"),
        ("Provider 协议", "anthropic") => Some("Anthropic Claude Messages 协议"),
        ("Provider 协议", "dashscope") => Some("阿里云百炼多模态 embedding 协议"),
        ("Provider 协议", "volcengine-ark-multimodal") => Some("火山方舟多模态 embedding 协议"),
        ("传输方式", "http") => Some("通过 HTTP 请求调用 provider，适合绝大多数模型服务"),
        ("传输方式", "websocket") => {
            Some("通过 WebSocket 长连接调用 provider，仅在供应商明确要求时使用")
        }
        ("推理强度" | "agent.reasoning_effort", "none") => Some("关闭 OpenAI 模型推理"),
        ("推理强度" | "agent.reasoning_effort", "minimal") => {
            Some("OpenAI 最小推理预算，延迟优先")
        }
        ("推理强度" | "agent.reasoning_effort", "low") => Some("较低推理预算，速度优先"),
        ("推理强度" | "agent.reasoning_effort", "medium") => Some("均衡推理预算"),
        ("推理强度" | "agent.reasoning_effort", "high") => Some("较高推理预算，适合复杂任务"),
        ("推理强度" | "agent.reasoning_effort", "xhigh") => {
            Some("扩展推理预算，仅在模型支持时使用")
        }
        ("推理强度" | "agent.reasoning_effort", "max") => {
            Some("Anthropic 最大推理预算，仅在模型支持时使用")
        }
        ("remote.mode", "standard") => Some("默认远控模式，适合常规消息频率"),
        ("remote.mode", "high-throughput") => Some("高吞吐远控模式，适合高频事件流"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_config::config_key_specs;

    #[test]
    fn every_setup_config_key_has_detail() {
        for spec in config_key_specs()
            .iter()
            .filter(|spec| !spec.setup_module.is_empty())
        {
            assert!(
                key_detail(spec.key).is_some(),
                "{} 缺少 setup 中文说明",
                spec.key
            );
        }
    }

    #[test]
    fn every_setup_config_option_has_detail() {
        for spec in config_key_specs()
            .iter()
            .filter(|spec| !spec.setup_module.is_empty())
        {
            for option in spec.options {
                assert!(
                    option_detail(spec.key, option).is_some(),
                    "{}={} 缺少可选值中文说明",
                    spec.key,
                    option
                );
            }
        }
    }
}
