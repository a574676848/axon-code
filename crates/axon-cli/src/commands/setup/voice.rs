//! `axon setup voice` 实现：读写独立 `[voice]` 段，复用 setup 的受锁事务。

use std::path::Path;

use axon_config::{
    AxonConfig, ConfigLoader, VoiceChannelConfig, VoiceConfig, VoiceModelConfig, to_toml_string,
};
use serde::Serialize;

use super::bootstrap::SetupTransaction;
use super::{env_path_for_config, format_error_result, resolved_setup_config_path};
use crate::management_commands::json_result;
use crate::{OutputFormat, VoiceSetupCommand};

/// 把参数意图落到候选配置上，并返回变更摘要。
fn apply_voice_mutation(
    config: &mut AxonConfig,
    command: &VoiceSetupCommand,
) -> Result<Vec<String>, String> {
    let mut changed = Vec::new();

    if command.reset {
        config.voice = VoiceConfig::default();
        changed.push("voice.reset".to_owned());
    }
    if let Some(enabled) = command.enable {
        config.voice.enabled = enabled;
        changed.push(format!("voice.enabled={enabled}"));
    }
    if let Some(lane) = command.lane {
        let value = lane.as_config_value();
        config.voice.channel_balancer = value.to_owned();
        changed.push(format!("voice.lane={value}"));
    }

    if let Some(name) = &command.channel {
        let index = match config
            .voice
            .channels
            .iter()
            .position(|item| item.name == *name)
        {
            Some(index) => index,
            None => {
                config.voice.channels.push(VoiceChannelConfig {
                    name: name.clone(),
                    ..VoiceChannelConfig::default()
                });
                config.voice.channels.len() - 1
            }
        };
        let channel = &mut config.voice.channels[index];
        if let Some(protocol) = command.protocol {
            channel.protocol = protocol.as_config_value().to_owned();
        }
        if let Some(base_url) = &command.base_url {
            channel.base_url = base_url.clone();
        }
        if let Some(transport) = command.wire_transport {
            channel.wire_transport = transport.as_config_value().to_owned();
        }
        if let Some(url) = &command.websocket_url {
            channel.websocket_url = Some(url.clone());
        }
        if let Some(env) = &command.api_key_env {
            channel.api_key_env = Some(env.clone());
        }
        changed.push(format!("voice.channel.{name}"));
    }

    if let Some(name) = &command.model {
        let channel = command
            .channel
            .clone()
            .or_else(|| config.voice.channels.first().map(|item| item.name.clone()))
            .ok_or_else(|| "配置语音模型前必须先用 --channel 指定渠道".to_owned())?;
        let index = match config
            .voice
            .models
            .iter()
            .position(|item| item.name == *name)
        {
            Some(index) => index,
            None => {
                config.voice.models.push(VoiceModelConfig {
                    name: name.clone(),
                    channel: channel.clone(),
                    ..VoiceModelConfig::default()
                });
                config.voice.models.len() - 1
            }
        };
        let model = &mut config.voice.models[index];
        if command.channel.is_some() {
            model.channel = channel.clone();
        }
        if let Some(id) = &command.id {
            model.id = id.clone();
        }
        if let Some(role) = command.role {
            model.role = role.as_config_value().to_owned();
        }
        if let Some(speaker) = &command.speaker {
            model.voice = speaker.clone();
        }
        if let Some(rate) = command.rate {
            model.speech_rate = rate;
        }
        if let Some(detection) = command.turn_detection {
            model.turn_detection = detection.as_config_value().to_owned();
        }
        changed.push(format!("voice.model.{name}"));
    }

    config
        .voice
        .validate()
        .map_err(|error| format!("语音配置校验失败：{error:?}"))?;
    Ok(changed)
}

/// 运行 `axon setup voice`。
pub(super) fn run_voice_setup(
    command: &VoiceSetupCommand,
    parent_format: OutputFormat,
    config_path: Option<&Path>,
) -> crate::CliRunResult {
    let format = command.format.unwrap_or(parent_format);
    let path = match resolved_setup_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return format_error_result(format, error, Vec::new()),
    };
    let loaded = match ConfigLoader::load_from_path(&path) {
        Ok(loaded) => loaded,
        Err(error) => return format_error_result(format, &format!("{error:?}"), Vec::new()),
    };

    let mut candidate = loaded.config.clone();
    let changed = if command.has_mutation() {
        match apply_voice_mutation(&mut candidate, command) {
            Ok(changed) => changed,
            Err(error) => return format_error_result(format, &error, Vec::new()),
        }
    } else {
        Vec::new()
    };

    let config_changed = candidate != loaded.config;
    if config_changed {
        let bytes = match to_toml_string(&candidate) {
            Ok(text) => text.into_bytes(),
            Err(error) => {
                return format_error_result(format, &format!("{error:?}"), Vec::new());
            }
        };
        let mut transaction = match SetupTransaction::begin(&path, &env_path_for_config(&path)) {
            Ok(transaction) => transaction,
            Err(error) => return format_error_result(format, &error, Vec::new()),
        };
        if let Err(error) = transaction.prepare(Some(&bytes), None) {
            transaction.abort_prepare();
            return format_error_result(format, &error, Vec::new());
        }
        if let Err(error) = transaction.commit() {
            let states = transaction.rollback();
            return format_error_result(format, &error, states);
        }
    }

    let report = VoiceSetupReport::new(&path, &candidate, changed, config_changed);
    match format {
        OutputFormat::Json => json_result(&report),
        OutputFormat::Text => crate::CliRunResult {
            exit_code: 0,
            stdout: render_text(&report),
            stderr: String::new(),
        },
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceSetupReport<'a> {
    status: &'static str,
    config_path: String,
    enabled: bool,
    lane: &'a str,
    channels: usize,
    models: usize,
    config_changed: bool,
    changed: Vec<String>,
    mode: &'static str,
    voice: &'a VoiceConfig,
    next_commands: Vec<String>,
    summary: Vec<String>,
}

impl<'a> VoiceSetupReport<'a> {
    fn new(
        path: &Path,
        config: &'a AxonConfig,
        changed: Vec<String>,
        config_changed: bool,
    ) -> Self {
        let mut next_commands = Vec::new();
        if !config.voice.enabled {
            next_commands.push("axon setup voice --enable".to_owned());
        }
        Self {
            status: "ok",
            config_path: path.display().to_string(),
            enabled: config.voice.enabled,
            lane: config.voice.lane(),
            channels: config.voice.channels.len(),
            models: config.voice.models.len(),
            config_changed,
            changed,
            mode: "local",
            voice: &config.voice,
            next_commands,
            summary: vec![format!(
                "语音配置车道 {}，渠道 {} 个，模型 {} 个",
                config.voice.lane(),
                config.voice.channels.len(),
                config.voice.models.len()
            )],
        }
    }
}

fn render_text(report: &VoiceSetupReport<'_>) -> String {
    let mut text = String::from("Axon 语音配置\n");
    text.push_str(&format!(
        "  状态：{}\n",
        if report.enabled {
            "已启用"
        } else {
            "未启用"
        }
    ));
    text.push_str(&format!("  车道：{}\n", report.lane));
    text.push_str(&format!(
        "  渠道：{} 个，模型：{} 个\n",
        report.channels, report.models
    ));
    text.push_str(&format!("  配置：{}\n", report.config_path));
    if !report.changed.is_empty() {
        text.push_str(&format!("  变更：{}\n", report.changed.join("，")));
    }
    for command in &report.next_commands {
        text.push_str(&format!("  下一步：{command}\n"));
    }
    for line in &report.summary {
        text.push_str(&format!("  {line}\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AxonCli, AxonCommand, SetupSubcommand, VoiceLaneArg, VoiceRoleArg, VoiceTurnDetectionArg,
    };

    fn voice_command(args: &[&str]) -> VoiceSetupCommand {
        let mut argv = vec!["axon", "setup", "voice"];
        argv.extend_from_slice(args);
        let cli = AxonCli::parse_from_args(argv);
        match cli.command {
            Some(AxonCommand::Setup(setup)) => match setup.command {
                Some(SetupSubcommand::Voice(voice)) => voice,
                _ => panic!("未解析到 voice 子命令"),
            },
            _ => panic!("未解析到 setup 命令"),
        }
    }

    #[test]
    fn voice_mutation_roundtrip_persists_channel_and_model() {
        let command = voice_command(&[
            "--enable",
            "--lane",
            "cascade",
            "--channel",
            "volc-tts",
            "--base-url",
            "https://example.invalid/v1",
            "--model",
            "seed-tts-2.0",
            "--role",
            "tts",
            "--speaker",
            "zh_female_example",
            "--rate",
            "20",
            "--turn-detection",
            "smart-turn",
        ]);
        let mut config = AxonConfig::default();
        let changed = apply_voice_mutation(&mut config, &command).unwrap();

        assert!(config.voice.is_enabled());
        assert_eq!(config.voice.lane(), "cascade");
        assert_eq!(config.voice.channels[0].name, "volc-tts");
        assert_eq!(
            config.voice.channels[0].base_url,
            "https://example.invalid/v1"
        );
        assert_eq!(config.voice.models[0].role, "tts");
        assert_eq!(config.voice.models[0].voice, "zh_female_example");
        assert_eq!(config.voice.models[0].speech_rate, 20);
        assert_eq!(config.voice.models[0].turn_detection, "smart_turn_v2");
        assert!(changed.iter().any(|item| item == "voice.channel.volc-tts"));

        let text = to_toml_string(&config).unwrap();
        let restored: AxonConfig = toml::from_str(&text).unwrap();
        assert_eq!(restored, config);
    }

    #[test]
    fn voice_mutation_rejects_tts_model_without_speaker() {
        let command = voice_command(&[
            "--channel",
            "volc-tts",
            "--model",
            "seed-tts-2.0",
            "--role",
            "tts",
        ]);
        let mut config = AxonConfig::default();
        assert!(apply_voice_mutation(&mut config, &command).is_err());
    }

    #[test]
    fn voice_enum_arguments_map_to_config_values() {
        assert_eq!(VoiceLaneArg::Realtime.as_config_value(), "realtime");
        assert_eq!(VoiceRoleArg::Asr.as_config_value(), "asr");
        assert_eq!(VoiceTurnDetectionArg::Manual.as_config_value(), "manual");
    }

    #[test]
    fn voice_read_without_mutation_changes_nothing() {
        let command = voice_command(&[]);
        assert!(!command.has_mutation());
        let config = AxonConfig::default();
        let mut candidate = config.clone();
        assert!(
            apply_voice_mutation(&mut candidate, &command)
                .unwrap()
                .is_empty()
        );
        assert_eq!(candidate, config);
    }
}
