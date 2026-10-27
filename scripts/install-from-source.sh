#!/usr/bin/env sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "${SCRIPT_DIR}/.." && pwd)
# 与发行版使用相同默认目录；构建和 JSON 读取都依赖 Node。
command -v node >/dev/null 2>&1 || { printf '%s\n' '未找到 Node.js。' >&2; exit 1; }
command -v npm >/dev/null 2>&1 || { printf '%s\n' '未找到 npm。' >&2; exit 1; }
cd "${REPO_ROOT}"
INSTALL_ROOT=${AXON_INSTALL_ROOT:-${HOME}/.axon/bin}
INSTALL_DIR=${AXON_INSTALL_DIR:-${INSTALL_ROOT}}
LAUNCHER_DIR=${INSTALL_ROOT}/launcher
LAUNCHER_BIN=${LAUNCHER_DIR}/axon-launcher
VERSIONS_DIR=${INSTALL_ROOT}/versions
CURRENT_FILE=${INSTALL_ROOT}/current.json
OLD_AXON_BIN=$(command -v axon || true)
STAGING_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/axon-source-install.XXXXXX")
DAEMON_STOPPED=0

cleanup() { rm -rf -- "${STAGING_ROOT}"; }
restore_daemon() {
    status=$?
    if [ "${status}" -ne 0 ] && [ "${DAEMON_STOPPED}" -eq 1 ] && [ -x "${LAUNCHER_BIN}" ]; then
        "${LAUNCHER_BIN}" daemon start >/dev/null 2>&1 || true
    fi
    cleanup
    exit "${status}"
}
trap restore_daemon EXIT

(cd "${REPO_ROOT}/crates/axon-desktop/ui" && npm ci && npm run build)

if command -v rustup >/dev/null 2>&1; then
    ACTIVE_TOOLCHAIN=$(CDPATH= cd -- "${REPO_ROOT}" && rustup show active-toolchain | awk '{print $1}')
    if [ -n "${ACTIVE_TOOLCHAIN}" ]; then
        rustup run "${ACTIVE_TOOLCHAIN}" cargo build --release --locked -p axon-cli -p axon-desktop -p axon-launcher -p axon-voice-host
    else
        cargo build --release --locked -p axon-cli -p axon-desktop -p axon-launcher -p axon-voice-host
    fi
else
    CARGO_BIN=$(command -v cargo || true)
    if [ -z "${CARGO_BIN}" ] && [ -x "${HOME}/.cargo/bin/cargo" ]; then CARGO_BIN="${HOME}/.cargo/bin/cargo"; fi
    if [ -z "${CARGO_BIN}" ]; then printf '%s\n' '未找到 cargo，请先安装 Rust 工具链。' >&2; exit 1; fi
    "${CARGO_BIN}" build --release --locked -p axon-cli -p axon-desktop -p axon-launcher
fi

RELEASE_BIN="${REPO_ROOT}/target/release"
CLI_BIN="${RELEASE_BIN}/axon"
DESKTOP_BIN="${RELEASE_BIN}/axon-desktop"
VOICE_HOST_BIN="${RELEASE_BIN}/axon-voice-host"
BUILT_LAUNCHER="${RELEASE_BIN}/axon-launcher"
for binary in "${CLI_BIN}" "${DESKTOP_BIN}" "${BUILT_LAUNCHER}" "${VOICE_HOST_BIN}"; do
    if [ ! -x "${binary}" ]; then printf '%s\n' "编译完成但缺少二进制：${binary}" >&2; exit 1; fi
done
"${CLI_BIN}" --version >/dev/null
"${DESKTOP_BIN}" --self-check >/dev/null

if [ -x "${LAUNCHER_BIN}" ]; then
    "${LAUNCHER_BIN}" daemon stop >/dev/null 2>&1 || true
    DAEMON_STOPPED=1
    attempts=0
    while "${LAUNCHER_BIN}" daemon status | node -e 'let s="";process.stdin.on("data",c=>s+=c);process.stdin.on("end",()=>{try{process.exit(JSON.parse(s).running?0:1)}catch{process.exit(1)}})'; do
        attempts=$((attempts + 1)); if [ "${attempts}" -ge 300 ]; then printf '%s\n' 'Daemon 停止超时。' >&2; exit 1; fi
        sleep 0.2
    done
fi
pkill -x axon-desktop >/dev/null 2>&1 || true

VERSION=$("${CLI_BIN}" --version | awk '{print $NF}')
RELEASE_ID="${VERSION}"
RELEASE_DIR="${VERSIONS_DIR}/${RELEASE_ID}"
STAGED_RELEASE="${VERSIONS_DIR}/.staging.$$"
mkdir -p "${STAGED_RELEASE}" "${LAUNCHER_DIR}" "${INSTALL_DIR}"
cp "${CLI_BIN}" "${STAGED_RELEASE}/axon"
cp "${DESKTOP_BIN}" "${STAGED_RELEASE}/axon-desktop"
cp "${VOICE_HOST_BIN}" "${STAGED_RELEASE}/axon-voice-host"
chmod 755 "${STAGED_RELEASE}/axon" "${STAGED_RELEASE}/axon-desktop" "${STAGED_RELEASE}/axon-voice-host"
rm -rf "${RELEASE_DIR}"
mv "${STAGED_RELEASE}" "${RELEASE_DIR}"
cp "${BUILT_LAUNCHER}" "${LAUNCHER_BIN}"
chmod 755 "${LAUNCHER_BIN}"
# 首次安装自动拉取语音运行时与模型；失败不阻塞安装。
VOICE_PROVISION="${REPO_ROOT}/scripts/voice/install-voice-runtime.sh"
if [ -f "${VOICE_PROVISION}" ]; then
    echo "[axon] 安装语音运行时与模型（首次约 750 MB）。"
    bash "${VOICE_PROVISION}" || echo "[axon] 语音运行时安装失败，语音功能暂不可用。" >&2
fiprintf '{"versionId":"%s"}\n' "${RELEASE_ID}" > "${INSTALL_ROOT}/.current.$$"
mv -f "${INSTALL_ROOT}/.current.$$" "${CURRENT_FILE}"
rm -f "${HOME}/.cargo/bin/axon" "${HOME}/.cargo/bin/axon-desktop"
cat > "${INSTALL_DIR}/axon" <<EOF
#!/usr/bin/env sh
exec "${LAUNCHER_BIN}" "\$@"
EOF
chmod 755 "${INSTALL_DIR}/axon"

# 固定版本目录只保留当前 CLI 版本，旧哈希 release 不再参与运行。
for release in "${VERSIONS_DIR}"/*; do
    [ -d "${release}" ] || continue
    release_id=$(basename "${release}")
    case "${release_id}" in
        .*) continue ;;
    esac
    [ "${release_id}" = "${RELEASE_ID}" ] || rm -rf -- "${release}"
done

PROFILE="${HOME}/.profile"
case "${SHELL:-sh}" in */bash) PROFILE="${HOME}/.bashrc";; */zsh) PROFILE="${ZDOTDIR:-${HOME}}/.zshrc";; */fish) PROFILE="${HOME}/.config/fish/config.fish";; esac
mkdir -p "$(dirname -- "${PROFILE}")"
TMP_PROFILE="${PROFILE}.axon.$$"
PATH_LINE="export PATH=\"${INSTALL_DIR}:\$PATH\""
case "${SHELL:-sh}" in */fish) PATH_LINE="fish_add_path \"${INSTALL_DIR}\"";; esac
if [ -f "${PROFILE}" ]; then grep -v '/axon/launcher' "${PROFILE}" | grep -vF "${PATH_LINE}" > "${TMP_PROFILE}" || true; else : > "${TMP_PROFILE}"; fi
printf '%s\n' "${PATH_LINE}" >> "${TMP_PROFILE}"
mv -f "${TMP_PROFILE}" "${PROFILE}"
export PATH="${INSTALL_DIR}:${PATH}"
CONFIG_PATH="${HOME}/.axon/config.toml"
if [ ! -s "${CONFIG_PATH}" ]; then
    "${LAUNCHER_BIN}" setup --provider-name openai --model gpt-4o --non-interactive --no-validate >/dev/null
fi
"${LAUNCHER_BIN}" daemon install
"${LAUNCHER_BIN}" daemon start
printf '%s\n' "[axon] 源码安装完成：${RELEASE_DIR}"
