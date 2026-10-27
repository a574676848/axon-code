//! 人格状态 HTTP Gateway 适配器。
//!
//! 只读：解析 `SOUL.md` 内核并从人格库读取状态、最近转移与已发布习得风格。
//! 库文件不存在时不创建，直接如实报告「尚未产生状态」。

use std::path::PathBuf;

use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use axon_persona::{
    PersonaCore, SoulDocument,
    domain::{Audience, HumorLevel, PersonaMode, Verbosity},
    infrastructure::{PERSONA_SCHEMA_IDENTITY, PersonaStore, persona_store_path},
};
use serde_json::{Value, json};

/// 人格路由前缀。
const PERSONA_PATH: &str = "/api/v1/persona";
/// 面板展示的最近转移条数。
const RECENT_TRANSITION_LIMIT: u32 = 10;
/// `SOUL.md` 文件名。
const SOUL_FILE_NAME: &str = "SOUL.md";
/// 发起读取的渠道标识，仅用于 principal 解析。
const DESKTOP_CHANNEL: &str = "desktop";

/// 人格状态网关适配器。
pub(crate) struct PersonaHttpAdapter {
    axon_root: PathBuf,
}

impl PersonaHttpAdapter {
    /// 以 Axon 用户目录构造适配器。
    pub(crate) fn new(axon_root: PathBuf) -> Self {
        Self { axon_root }
    }

    fn summary(&self) -> Value {
        json!({
            "soul": self.soul_summary(),
            "owner": self.owner_summary(),
            "state": self.state_summary(),
            "transitions": self.transitions_summary(),
            "style": self.style_summary(),
            "database": self.database_summary(),
        })
    }

    fn soul_summary(&self) -> Value {
        let path = self.axon_root.join(SOUL_FILE_NAME);
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) => {
                return json!({
                    "file": SOUL_FILE_NAME,
                    "ready": false,
                    "error": format!("读取人设文件失败：{error}"),
                });
            }
        };
        match SoulDocument::parse(&raw) {
            Ok(document) => {
                let core = &document.core;
                json!({
                    "file": SOUL_FILE_NAME,
                    "ready": true,
                    "error": Value::Null,
                    "schema": document.schema,
                    "identity": {
                        "name": core.identity.name,
                        "address": core.identity.address,
                        "role": core.identity.role,
                    },
                    "backstoryChars": core.backstory.chars().count(),
                    "invariants": core.invariants,
                    "traits": core.traits,
                    "voice": {
                        "warmth": core.voice.warmth,
                        "formality": core.voice.formality,
                        "humor": core.voice.humor.as_token(),
                        "verbosity": core.voice.verbosity.as_token(),
                    },
                    "rules": rules_json(core),
                    "notesChars": core.notes.as_ref().map_or(0, |notes| notes.chars().count()),
                })
            }
            Err(error) => json!({
                "file": SOUL_FILE_NAME,
                "ready": false,
                "error": error.to_string(),
            }),
        }
    }

    fn owner_summary(&self) -> Value {
        match crate::runtime_principal_context::load_run_principal(&self.axon_root, DESKTOP_CHANNEL)
        {
            Ok(Some(principal)) => json!({
                "present": true,
                "owner": principal.owner_id.as_str(),
            }),
            Ok(None) => json!({"present": false}),
            Err(error) => json!({"present": false, "error": error}),
        }
    }

    fn owner_id(&self) -> Option<String> {
        crate::runtime_principal_context::load_run_principal(&self.axon_root, DESKTOP_CHANNEL)
            .ok()
            .flatten()
            .map(|principal| principal.owner_id.as_str().to_owned())
    }

    fn open_store(&self) -> Option<Result<PersonaStore, String>> {
        let path = persona_store_path(&self.axon_root);
        if !path.exists() {
            return None;
        }
        Some(PersonaStore::open_read_only(&path).map_err(|error| error.to_string()))
    }

    fn state_summary(&self) -> Value {
        let Some(owner_id) = self.owner_id() else {
            return Value::Null;
        };
        let Some(store) = self.open_store() else {
            return Value::Null;
        };
        let Ok(store) = store else {
            return Value::Null;
        };
        match store.load_state(&owner_id) {
            Ok(Some(state)) => json!({
                "revision": state.revision,
                "warmthOffset": state.warmth_offset,
                "familiarity": state.familiarity,
                "userAffect": state.user_affect.as_token(),
                "failureStreak": state.failure_streak,
                "lastCause": state.last_cause,
                "driftGuard": state.drift_guard,
                "updatedAtMs": state.updated_at_ms,
            }),
            Ok(None) | Err(_) => Value::Null,
        }
    }

    fn transitions_summary(&self) -> Value {
        let Some(owner_id) = self.owner_id() else {
            return json!([]);
        };
        let Some(Ok(store)) = self.open_store() else {
            return json!([]);
        };
        match store.recent_transitions(&owner_id, RECENT_TRANSITION_LIMIT) {
            Ok(records) => json!(
                records
                    .into_iter()
                    .map(|record| json!({
                        "revision": record.revision,
                        "cause": record.cause,
                        "occurredAtMs": record.occurred_at_ms,
                    }))
                    .collect::<Vec<_>>()
            ),
            Err(_) => json!([]),
        }
    }

    fn style_summary(&self) -> Value {
        let Some(owner_id) = self.owner_id() else {
            return json!({"revision": 0, "entries": []});
        };
        let Some(Ok(store)) = self.open_store() else {
            return json!({"revision": 0, "entries": []});
        };
        match store.published_style(&owner_id) {
            Ok(published) => json!({
                "revision": published.revision,
                "entries": published
                    .entries
                    .into_iter()
                    .map(|entry| json!({
                        "styleId": entry.style_id,
                        "text": entry.text,
                        "publishedAtMs": entry.published_at_ms,
                    }))
                    .collect::<Vec<_>>(),
            }),
            Err(_) => json!({"revision": 0, "entries": []}),
        }
    }

    fn database_summary(&self) -> Value {
        let path = persona_store_path(&self.axon_root);
        let bytes = database_bytes(&path);
        json!({
            "ready": bytes.is_some(),
            "schema": PERSONA_SCHEMA_IDENTITY,
            "bytes": bytes.unwrap_or(0),
        })
    }
}

impl GatewayHttpRouteHandler for PersonaHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        _body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        let suffix = path.strip_prefix(PERSONA_PATH)?;
        let parts = suffix
            .trim_matches('/')
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        match (method, parts.as_slice()) {
            ("GET", ["summary"]) => Some(GatewayHttpResponse::json(200, &self.summary())),
            ("GET", ["portrait", name @ ..]) => Some(self.portrait(name)),
            _ => None,
        }
    }
}

impl PersonaHttpAdapter {
    /// 读取形象资产：只暴露 `persona/` 下的白名单图片，避免任意路径读取。
    fn portrait(&self, rest: &[&str]) -> GatewayHttpResponse {
        let name = rest.join("/");
        if name.is_empty() || name.contains("..") || !name.ends_with(".png") {
            return GatewayHttpResponse::json(404, &json!({"error": "portrait_not_found"}));
        }
        let path = self.axon_root.join("persona").join(&name);
        let Ok(bytes) = std::fs::read(&path) else {
            return GatewayHttpResponse::json(404, &json!({"error": "portrait_not_found"}));
        };
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        GatewayHttpResponse::json(
            200,
            &json!({"file": name, "mime": "image/png", "base64": encoded}),
        )
    }
}

fn rules_json(core: &PersonaCore) -> Value {
    json!(
        core.rules
            .iter()
            .map(|rule| json!({
                "mode": rule.when.mode.map(PersonaMode::as_token),
                "audience": rule.when.audience.map(Audience::as_token),
                "userAffect": rule.when.user_affect.map(|affect| affect.as_token()),
                "warmth": rule.voice.warmth,
                "formality": rule.voice.formality,
                "humor": rule.voice.humor.map(HumorLevel::as_token),
                "verbosity": rule.voice.verbosity.map(Verbosity::as_token),
            }))
            .collect::<Vec<_>>()
    )
}

fn database_bytes(path: &std::path::Path) -> Option<u64> {
    let main = std::fs::metadata(path).ok()?.len();
    let wal = std::fs::metadata(format!("{}-wal", path.display()))
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    Some(main.saturating_add(wal))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portrait_route_returns_base64_for_existing_asset() {
        let root = axon_test_support::test_temp_dir();
        let dir = root.path().join("persona");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("art-halfbody.png"), [0x89, b'P', b'N', b'G']).unwrap();
        let adapter = PersonaHttpAdapter::new(root.path().to_path_buf());

        let response = adapter
            .handle_request(
                "GET",
                "/api/v1/persona/portrait/art-halfbody.png",
                None,
                b"",
            )
            .expect("路由必须命中");

        assert_eq!(response.status, 200);
        let body = String::from_utf8_lossy(&response.body);
        assert!(body.contains("\"mime\":\"image/png\""));
        assert!(body.contains("\"file\":\"art-halfbody.png\""));
    }

    #[test]
    fn portrait_route_rejects_traversal_and_missing_assets() {
        let root = axon_test_support::test_temp_dir();
        let adapter = PersonaHttpAdapter::new(root.path().to_path_buf());

        for path in [
            "/api/v1/persona/portrait/missing.png",
            "/api/v1/persona/portrait/../config.toml",
            "/api/v1/persona/portrait/character.md",
        ] {
            let response = adapter
                .handle_request("GET", path, None, b"")
                .expect("路由必须命中");
            assert_eq!(response.status, 404, "{path}");
        }
    }
}
