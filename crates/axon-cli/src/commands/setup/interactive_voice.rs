//! `axon setup` TUI 语音页：读取并编辑独立 `[voice]` 段。
//!
//! 本模块只做输入层：收集到的参数填进 [`VoiceSetupCommand`]，交回 `run_setup`
//! 走同一条 plan / confirm / SetupTransaction 链路，落盘逻辑始终只有一份。

use axon_config::{AxonConfig, VoiceChannelConfig, VoiceModelConfig};

use crate::{VoiceRoleArg, VoiceSetupCommand, VoiceTurnDetectionArg};

use super::{
    setup_interactive_canvas::render_setup_frame,
    setup_interactive_prompts::{prompt_choice, prompt_i32, prompt_string},
    setup_interactive_style::{SetupStyle, paint},
};

const ROLES: &[&str] = &["asr", "tts", "realtime"];
const TURNS: &[&str] = &["server_vad", "smart_turn_v2", "manual"];

/// Esc 或非候选值一律回落到当前值，避免误改。
fn pick(current: &str, next: String, allowed: &[&str]) -> String {
    if allowed.contains(&next.as_str()) {
        next
    } else {
        current.to_owned()
    }
}

fn role_arg(value: &str) -> VoiceRoleArg {
    match value {
        "tts" => VoiceRoleArg::Tts,
        "realtime" => VoiceRoleArg::Realtime,
        _ => VoiceRoleArg::Asr,
    }
}

fn turn_arg(value: &str) -> VoiceTurnDetectionArg {
    match value {
        "smart_turn_v2" => VoiceTurnDetectionArg::SmartTurn,
        "manual" => VoiceTurnDetectionArg::Manual,
        _ => VoiceTurnDetectionArg::ServerVad,
    }
}

fn ensure_channel(cmd: &mut VoiceSetupCommand, name: &str) {
    if cmd.channel.is_none() && !name.is_empty() {
        cmd.channel = Some(name.to_owned());
    }
}

fn ensure_model(cmd: &mut VoiceSetupCommand, name: &str) {
    if cmd.model.is_none() && !name.is_empty() {
        cmd.model = Some(name.to_owned());
    }
}

/// 语音页仪表盘：一屏看清当前 `[voice]` 段。
pub(super) fn print_voice_dashboard(config: &AxonConfig) {
    let voice = &config.voice;
    let mut lines = vec![
        paint(SetupStyle::Muted, "主菜单  ›  语音"),
        String::new(),
        format!(
            "  状态   : {}",
            if voice.is_available() {
                "已配置"
            } else {
                "未配置"
            }
        ),
        format!("  渠道   : {}", voice.channels.len()),
        format!("  模型   : {}", voice.models.len()),
    ];
    if let Some(channel) = voice.channels.first() {
        lines.push(format!(
            "    - {} @ {} ({})",
            channel.name, channel.base_url, channel.wire_transport
        ));
    }
    if let Some(model) = voice.models.first() {
        lines.push(format!(
            "    - {} [{}] voice={} rate={}",
            model.name, model.role, model.voice, model.speech_rate
        ));
    }
    lines.extend([
        String::new(),
        paint(SetupStyle::Muted, "逐项回车保留原值；Esc 返回主菜单"),
    ]);
    render_setup_frame(&paint(SetupStyle::Accent, "VOICE CONFIG"), &lines);
}

/// 渠道级输入。仅当值真正变化才写 `cmd`，并保证同时带上渠道名。
fn collect_channel_input(current: Option<&VoiceChannelConfig>, cmd: &mut VoiceSetupCommand) {
    let name_now = current.map_or_else(String::new, |c| c.name.clone());
    let name = prompt_string("渠道名称", "语音渠道名，如 volc-tts；回车保留", &name_now);
    if name != name_now && !name.is_empty() {
        cmd.channel = Some(name.clone());
    }

    let base_now = current.map_or_else(String::new, |c| c.base_url.clone());
    let base = prompt_string("API 地址", "语音服务基址；回车保留", &base_now);
    if base != base_now && !name.is_empty() {
        ensure_channel(cmd, &name);
        cmd.base_url = Some(base);
    }

    let ws_now = current
        .and_then(|c| c.websocket_url.clone())
        .unwrap_or_default();
    let ws = prompt_string("WebSocket 地址", "留空则由 API 地址推导；回车保留", &ws_now);
    if ws != ws_now && !name.is_empty() {
        ensure_channel(cmd, &name);
        cmd.websocket_url = Some(ws);
    }

    let env_now = current
        .and_then(|c| c.api_key_env.clone())
        .unwrap_or_default();
    let env = prompt_string(
        "密钥环境变量名",
        "仅变量名，不收密钥明文；回车保留",
        &env_now,
    );
    if env != env_now && !name.is_empty() {
        ensure_channel(cmd, &name);
        cmd.api_key_env = Some(env);
    }

    let priority_now = current.map_or(0, |c| c.priority);
    let priority = prompt_i32("渠道优先级", "数值越大越优先；回车保留", priority_now);
    if priority != priority_now && !name.is_empty() {
        ensure_channel(cmd, &name);
        cmd.channel_priority = Some(priority);
    }

    let weight_now = current.map_or(1, |c| c.weight);
    let weight_now_i32 = i32::try_from(weight_now).unwrap_or(i32::MAX);
    let weight = prompt_i32(
        "渠道权重",
        "同优先级内越大越优先（≥1）；回车保留",
        weight_now_i32,
    );
    if weight != weight_now_i32 && !name.is_empty() {
        ensure_channel(cmd, &name);
        cmd.channel_weight = u32::try_from(weight.max(1)).ok();
    }
}

/// 模型级输入。仅当值真正变化才写 `cmd`，并保证同时带上模型名。
fn collect_model_input(current: Option<&VoiceModelConfig>, cmd: &mut VoiceSetupCommand) {
    let name_now = current.map_or_else(String::new, |m| m.name.clone());
    let name = prompt_string("模型名称", "语音模型路由名；回车保留", &name_now);
    if name != name_now && !name.is_empty() {
        cmd.model = Some(name.clone());
    }

    let id_now = current.map_or_else(String::new, |m| m.id.clone());
    let id = prompt_string("上游模型标识", "供应商侧模型 id；回车保留", &id_now);
    if id != id_now && !name.is_empty() {
        ensure_model(cmd, &name);
        cmd.id = Some(id);
    }

    let role_now = current.map_or("asr", |m| m.role.as_str());
    let role = pick(
        role_now,
        prompt_choice(
            "模型角色",
            "asr 识别 / tts 合成 / realtime 实时",
            ROLES,
            role_now,
        ),
        ROLES,
    );
    if role != role_now && !name.is_empty() {
        ensure_model(cmd, &name);
        cmd.role = Some(role_arg(&role));
    }

    let speaker_now = current.map_or_else(String::new, |m| m.voice.clone());
    let speaker = prompt_string("音色标识", "tts/realtime 必填；回车保留", &speaker_now);
    if speaker != speaker_now && !name.is_empty() {
        ensure_model(cmd, &name);
        cmd.speaker = Some(speaker);
    }

    let rate_now = current.map_or(0, |m| m.speech_rate);
    let rate = prompt_i32("语速", "-50 到 100；回车保留", rate_now);
    if rate != rate_now && !name.is_empty() {
        ensure_model(cmd, &name);
        cmd.rate = Some(rate);
    }

    let turn_now = current.map_or("server_vad", |m| m.turn_detection.as_str());
    let turn = pick(
        turn_now,
        prompt_choice("轮次检测", "服务端 VAD / 语义轮次 / 手动", TURNS, turn_now),
        TURNS,
    );
    if turn != turn_now && !name.is_empty() {
        ensure_model(cmd, &name);
        cmd.turn_detection = Some(turn_arg(&turn));
    }

    let priority_now = current.map_or(0, |m| m.priority);
    let priority = prompt_i32("模型优先级", "数值越大越优先；回车保留", priority_now);
    if priority != priority_now && !name.is_empty() {
        ensure_model(cmd, &name);
        cmd.model_priority = Some(priority);
    }

    let weight_now = current.map_or(1, |m| m.weight);
    let weight_now_i32 = i32::try_from(weight_now).unwrap_or(i32::MAX);
    let weight = prompt_i32(
        "模型权重",
        "同优先级内越大越优先（≥1）；回车保留",
        weight_now_i32,
    );
    if weight != weight_now_i32 && !name.is_empty() {
        ensure_model(cmd, &name);
        cmd.model_weight = u32::try_from(weight.max(1)).ok();
    }
}

/// 进入 TUI 语音页，收集参数后返回可被 `run_setup` 复用的命令。
pub(super) fn collect_voice_input(config: &AxonConfig) -> VoiceSetupCommand {
    print_voice_dashboard(config);
    let voice = &config.voice;
    let mut cmd = VoiceSetupCommand::default();

    collect_channel_input(voice.channels.first(), &mut cmd);
    collect_model_input(voice.models.first(), &mut cmd);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_config::voice::{
        VOICE_ROLE_TTS, VOICE_TURN_MANUAL, VOICE_TURN_SERVER_VAD, VOICE_TURN_SMART_TURN,
    };

    #[test]
    fn escape_and_unknown_values_fall_back_to_current() {
        assert_eq!(pick("asr", "escape".to_owned(), ROLES), "asr");
        assert_eq!(pick("tts", "bogus".to_owned(), ROLES), "tts");
        assert_eq!(pick("realtime", "tts".to_owned(), ROLES), "tts");
    }

    #[test]
    fn enum_mapping_matches_config_constants() {
        assert_eq!(role_arg("tts").as_config_value(), VOICE_ROLE_TTS);
        assert_eq!(
            turn_arg("server_vad").as_config_value(),
            VOICE_TURN_SERVER_VAD
        );
        assert_eq!(
            turn_arg("smart_turn_v2").as_config_value(),
            VOICE_TURN_SMART_TURN
        );
        assert_eq!(turn_arg("manual").as_config_value(), VOICE_TURN_MANUAL);
    }

    #[test]
    fn channel_and_model_name_are_set_once() {
        let mut cmd = VoiceSetupCommand::default();
        ensure_channel(&mut cmd, "volc-tts");
        ensure_channel(&mut cmd, "other");
        assert_eq!(cmd.channel.as_deref(), Some("volc-tts"));
        ensure_model(&mut cmd, "seed-tts");
        ensure_model(&mut cmd, "other");
        assert_eq!(cmd.model.as_deref(), Some("seed-tts"));
    }

    #[test]
    fn empty_name_never_creates_a_channel_or_model() {
        let mut cmd = VoiceSetupCommand::default();
        ensure_channel(&mut cmd, "");
        ensure_model(&mut cmd, "");
        assert!(cmd.channel.is_none());
        assert!(cmd.model.is_none());
    }
}
