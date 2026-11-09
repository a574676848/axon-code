//! 运行态装配与切换语义的断言。全部走内存文档，不碰真实状态文件。

use super::Runtime;
use super::store::{
    ClientLayer, ClientStateDocument, LOCAL_PROFILE_ID, ProfileProjection, ProfileSection,
};
use crate::connection::profile::{BackendProfile, CapsState, CredentialSource};
use crate::state::RegisteredWorkspace;

fn document_with_one_local_profile() -> ClientStateDocument {
    let mut document = ClientStateDocument::default();
    document.client.last_profile_id = Some(LOCAL_PROFILE_ID.to_owned());
    document.profiles.insert(
        LOCAL_PROFILE_ID.to_owned(),
        ProfileSection {
            meta: BackendProfile::local("我的开发机"),
            projection: ProfileProjection::default(),
        },
    );
    document
}

#[test]
fn active_profile_and_label_come_from_the_document() {
    let runtime = Runtime::from_document(document_with_one_local_profile(), false);
    assert_eq!(
        runtime.active_profile_id().as_deref(),
        Some(LOCAL_PROFILE_ID)
    );
    let active = runtime.active().expect("应有生效档案");
    // 本机档案的显示名由代码决定：状态文件里留着的旧名字（例如主机名派生的
    // 「我的开发机」）不会漏到界面上。
    assert_eq!(super::display_label(&active.profile), "本机");
    assert_eq!(runtime.profiles().len(), 1);
}

#[test]
fn blank_cloud_labels_fall_back_to_a_visible_placeholder() {
    let profile = BackendProfile::cloud(
        "   ",
        "https://axon.example.com",
        "wss://axon.example.com",
        CredentialSource::SessionOnly,
    );
    assert_eq!(super::display_label(&profile), "未命名档案");
}

#[test]
fn local_profile_label_is_fixed_by_the_app() {
    // 本机档案就叫「本机」：状态文件里留着的旧名字（主机名派生的「我的开发机」）
    // 不会漏到界面上，也不需要用户为了改个称呼去编辑状态文件。
    assert_eq!(super::display_label(&BackendProfile::local("本机")), "本机");
    assert_eq!(
        super::display_label(&BackendProfile::local("我的开发机")),
        "本机"
    );
}

#[test]
fn caps_are_overridden_by_profile_mode() {
    let mut document = document_with_one_local_profile();
    document
        .profiles
        .get_mut(LOCAL_PROFILE_ID)
        .expect("档案存在")
        .meta = BackendProfile::cloud(
        "公司云",
        "https://axon.example.com",
        "wss://axon.example.com",
        CredentialSource::SessionOnly,
    );
    let runtime = Runtime::from_document(document, false);
    runtime.set_caps(CapsState::Known(super::profile::KnownCaps {
        set: crate::connection::profile::CapabilitySet {
            terminal: true,
            service_control: true,
            ..crate::connection::profile::CapabilitySet::default()
        },
    }));
    let effective = runtime.effective_caps().set().expect("已知能力位");
    assert!(!effective.terminal);
    assert!(!effective.service_control);
}

#[test]
fn projection_round_trips_through_the_runtime() {
    let runtime = Runtime::from_document(document_with_one_local_profile(), false);
    let mut projection = runtime.projection(LOCAL_PROFILE_ID);
    projection.workspaces.push(RegisteredWorkspace {
        path: "D:/work/demo".to_owned(),
        name: "demo".to_owned(),
    });
    // 测试装配不持有落盘根，因此这一步只改内存：它绝不允许把夹具写进用户的状态文件。
    runtime
        .set_projection(LOCAL_PROFILE_ID, projection)
        .expect("内存装配必须允许写入");
    let stored = runtime.projection(LOCAL_PROFILE_ID);
    assert_eq!(stored.workspaces.len(), 1);
    assert_eq!(stored.workspaces[0].path, "D:/work/demo");
}

/// 测试装配不得落真实路径：这是回归护栏，防止夹具内容整份覆盖用户的状态文件。
#[test]
fn test_runtime_never_holds_a_persist_root() {
    let runtime = Runtime::from_document(document_with_one_local_profile(), false);
    assert!(runtime.read().root.is_none(), "测试装配不得持有落盘根");
    assert!(
        runtime
            .update_document(|document| {
                document.client.theme = Some("forest".to_owned());
                Ok(())
            })
            .is_ok(),
        "只改内存的写入必须成功"
    );
    assert_eq!(
        runtime.read().document.client.theme.as_deref(),
        Some("forest")
    );
}

#[test]
fn read_only_runtime_refuses_every_write() {
    let runtime = Runtime::from_document(document_with_one_local_profile(), true);
    assert!(runtime.is_read_only());

    let error = runtime
        .update_document(|document| {
            document.client.theme = Some("forest".to_owned());
            Ok(())
        })
        .expect_err("只读模式必须拒绝写入");
    assert!(error.contains("只读"), "{error}");

    assert!(runtime.activate_profile(LOCAL_PROFILE_ID).is_err());
    assert!(runtime.rollback_profile().is_err());
}

#[test]
fn activating_an_unknown_profile_is_refused() {
    let runtime = Runtime::from_document(document_with_one_local_profile(), true);
    let error = runtime
        .activate_profile("不存在的档案")
        .expect_err("未知档案必须被拒绝");
    assert!(error.contains("不存在"), "{error}");
}

#[test]
fn session_secret_is_kept_in_memory_only_for_session_profiles() {
    let mut document = ClientStateDocument::default();
    document.client.last_profile_id = Some("cloud-1".to_owned());
    document.profiles.insert(
        "cloud-1".to_owned(),
        ProfileSection {
            meta: BackendProfile::cloud(
                "公司云",
                "https://axon.example.com",
                "wss://axon.example.com",
                CredentialSource::SessionOnly,
            ),
            projection: ProfileProjection::default(),
        },
    );
    let runtime = Runtime::from_document(document, false);
    assert_eq!(runtime.profile_api_token(), None);
    runtime.set_session_secret(Some("sk-session".to_owned()));
    assert_eq!(runtime.profile_api_token().as_deref(), Some("sk-session"));
    runtime.set_session_secret(None);
    assert_eq!(runtime.profile_api_token(), None);
}

#[test]
fn local_env_file_profiles_never_return_a_keychain_token() {
    let runtime = Runtime::from_document(document_with_one_local_profile(), false);
    assert_eq!(
        runtime.credential_source(),
        Some(CredentialSource::LocalEnvFile)
    );
    assert_eq!(runtime.profile_api_token(), None, "本机档案的凭据只走 .env");
}

#[test]
fn client_layer_defaults_keep_notifications_on() {
    let layer = ClientLayer::default();
    assert!(layer.notifications_enabled);
    assert_eq!(layer.onboarding_version, 0, "0 表示尚未完成引导");
    assert_eq!(layer.theme, None);
}
