#!/usr/bin/env sh
# Axon 一键安装脚本
# 用法：curl -fsSL https://raw.githubusercontent.com/a574676848/axon-code/master/scripts/install.sh | sh
# 或者指定安装目录：
#   curl -fsSL https://raw.githubusercontent.com/a574676848/axon-code/master/scripts/install.sh | AXON_INSTALL_DIR="${HOME}/.axon/bin" sh
# 安装完成后同时安装并启动 Daemon 用户服务（不要求预先配置 Remote/Feishu）。
set -eu

REPO="a574676848/axon-code"
BINARY_NAME="axon"
SKILL_NAME="operate-axon"
BUNDLED_SKILLS_DIRECTORY="docs/skills"
BUNDLED_SOUL_PATH="assets/defaults/SOUL.md"
DEFAULT_INSTALL_ROOT="${HOME}/.axon/bin"
INSTALL_ROOT="${AXON_INSTALL_ROOT:-${DEFAULT_INSTALL_ROOT}}"
DEFAULT_INSTALL_DIR="${INSTALL_ROOT}"
INSTALL_DIR="${AXON_INSTALL_DIR:-${DEFAULT_INSTALL_DIR}}"
VERSIONS_DIR="${INSTALL_ROOT}/versions"
CURRENT_FILE="${INSTALL_ROOT}/current.json"
LAUNCHER_DIR="${INSTALL_ROOT}/launcher"
LAUNCHER_PATH="${LAUNCHER_DIR}/axon-launcher"
INSTALL_LOCK_DIR="${INSTALL_ROOT}/install.lock"
AXON_USER_DIR="${HOME}/.axon"
SKILLS_DIR="${AXON_USER_DIR}/skills"
SOUL_FILE="${AXON_USER_DIR}/SOUL.md"
IDENTITY_DIR="${AXON_USER_DIR}/identity"
OWNER_FILE="${IDENTITY_DIR}/owner.json"
OWNER_SCHEMA_VERSION="1"
CONFIG_PATH="${AXON_CONFIG_PATH:-}"
GITHUB_API="https://api.github.com/repos/${REPO}/releases/latest"

# ── 颜色输出 ─────────────────────────────────────────────────────────────────

RESET=""
BOLD=""
GREEN=""
YELLOW=""
RED=""
CYAN=""

if [ -t 1 ] && command -v tput >/dev/null 2>&1; then
    RESET="$(tput sgr0)"
    BOLD="$(tput bold)"
    GREEN="$(tput setaf 2)"
    YELLOW="$(tput setaf 3)"
    RED="$(tput setaf 1)"
    CYAN="$(tput setaf 6)"
fi

info()    { printf "${BOLD}${CYAN}[axon]${RESET} %s\n" "$*"; }
success() { printf "${BOLD}${GREEN}[axon]${RESET} %s\n" "$*"; }
warn()    { printf "${BOLD}${YELLOW}[axon]${RESET} %s\n" "$*" >&2; }
die()     { printf "${BOLD}${RED}[axon]${RESET} %s\n" "$*" >&2; exit 1; }

acquire_install_lock() {
    mkdir -p "${INSTALL_ROOT}"
    while ! mkdir "${INSTALL_LOCK_DIR}" 2>/dev/null; do
        sleep 1
    done
}

release_install_lock() {
    rmdir "${INSTALL_LOCK_DIR}" 2>/dev/null || true
}

# ── 平台检测 ──────────────────────────────────────────────────────────────────

detect_os() {
    case "$(uname -s)" in
        Linux*)  echo "linux"  ;;
        Darwin*) echo "darwin" ;;
        *)       die "不支持的操作系统：$(uname -s)，请访问 https://github.com/${REPO}/releases 手动下载。" ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)    echo "x86_64" ;;
        aarch64|arm64)   echo "aarch64" ;;
        armv7l)          echo "armv7" ;;
        *) die "不支持的 CPU 架构：$(uname -m)" ;;
    esac
}

detect_target() {
    OS="$(detect_os)"
    ARCH="$(detect_arch)"
    case "${OS}-${ARCH}" in
        darwin-aarch64)  echo "aarch64-apple-darwin" ;;
        darwin-x86_64)   echo "x86_64-apple-darwin" ;;
        linux-x86_64)    echo "x86_64-unknown-linux-gnu" ;;
        linux-aarch64)   echo "aarch64-unknown-linux-gnu" ;;
        *) die "当前平台 ${OS}-${ARCH} 暂无预编译二进制，请访问 https://github.com/${REPO}/releases 手动下载。" ;;
    esac
}

# ── 工具检查 ──────────────────────────────────────────────────────────────────

need_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        die "需要 '$1' 命令但未找到，请先安装后重试。"
    fi
}

check_sha256() {
    FILE="$1"
    EXPECTED="$2"
    if command -v sha256sum >/dev/null 2>&1; then
        ACTUAL="$(sha256sum "${FILE}" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
        ACTUAL="$(shasum -a 256 "${FILE}" | awk '{print $1}')"
    else
        warn "未找到 sha256sum/shasum，跳过校验（不推荐）。"
        return 0
    fi
    if [ "${ACTUAL}" != "${EXPECTED}" ]; then
        die "SHA256 校验失败！\n  期望：${EXPECTED}\n  实际：${ACTUAL}\n请重新运行安装脚本。"
    fi
}

# ── 版本获取 ──────────────────────────────────────────────────────────────────

get_latest_version() {
    # 优先 curl，其次 wget
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 "${GITHUB_API}" 2>/dev/null \
            | grep '"tag_name"' \
            | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\([^"]*\)".*/\1/'
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- "${GITHUB_API}" 2>/dev/null \
            | grep '"tag_name"' \
            | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\([^"]*\)".*/\1/'
    else
        die "需要 curl 或 wget 来下载 Axon。"
    fi
}

download_file() {
    URL="$1"
    DEST="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 --progress-bar "${URL}" -o "${DEST}"
    else
        wget -q --show-progress "${URL}" -O "${DEST}"
    fi
}

install_bundled_skill() {
    PACKAGE_ROOT="$1"
    SKILL_SOURCE="${PACKAGE_ROOT}/${BUNDLED_SKILLS_DIRECTORY}/${SKILL_NAME}"
    SKILL_DEST="${SKILLS_DIR}/${SKILL_NAME}"
    SKILL_STAGING="${SKILLS_DIR}/.${SKILL_NAME}.tmp.$$"

    if [ ! -f "${SKILL_SOURCE}/SKILL.md" ]; then
        warn "发行包未包含 ${SKILL_NAME} Skill，跳过 Skill 安装。"
        return 0
    fi

    mkdir -p "${SKILLS_DIR}"
    rm -rf "${SKILL_STAGING}"
    cp -R "${SKILL_SOURCE}" "${SKILL_STAGING}"
    rm -rf "${SKILL_DEST}"
    mv "${SKILL_STAGING}" "${SKILL_DEST}"
    success "${SKILL_NAME} Skill 已安装到 ${SKILL_DEST}"
}

install_bundled_soul() {
    PACKAGE_ROOT="$1"
    SOUL_SOURCE="${PACKAGE_ROOT}/${BUNDLED_SOUL_PATH}"

    if [ -e "${SOUL_FILE}" ] || [ -L "${SOUL_FILE}" ]; then
        info "保留现有 Soul：${SOUL_FILE}"
        return 0
    fi
    if [ ! -f "${SOUL_SOURCE}" ]; then
        warn "发行包未包含默认 SOUL.md，跳过 Soul 初始化。"
        return 0
    fi

    mkdir -p "${AXON_USER_DIR}"
    cp "${SOUL_SOURCE}" "${SOUL_FILE}"
    chmod 600 "${SOUL_FILE}"
    success "默认 Soul 已创建：${SOUL_FILE}"
}

initialize_owner_identity() {
    if [ -e "${OWNER_FILE}" ] || [ -L "${OWNER_FILE}" ]; then
        info "保留现有 owner identity：${OWNER_FILE}"
        return 0
    fi
    mkdir -p "${IDENTITY_DIR}"
    if command -v uuidgen >/dev/null 2>&1; then
        OWNER_UUID="$(uuidgen | tr '[:upper:]' '[:lower:]')"
    elif [ -r /proc/sys/kernel/random/uuid ]; then
        OWNER_UUID="$(cat /proc/sys/kernel/random/uuid)"
    else
        OWNER_HEX="$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
        OWNER_UUID="$(printf '%s' "${OWNER_HEX}" | sed 's/^\(........\)\(....\)\(....\)\(....\)\(............\)$/\1-\2-\3-\4-\5/')"
    fi
    OWNER_STAGING="${OWNER_FILE}.tmp.$$"
    printf '{"schemaVersion":%s,"ownerId":"owner_%s"}\n' \
        "${OWNER_SCHEMA_VERSION}" "${OWNER_UUID}" > "${OWNER_STAGING}"
    chmod 600 "${OWNER_STAGING}"
    mv -f "${OWNER_STAGING}" "${OWNER_FILE}"
    success "owner identity 已创建：${OWNER_FILE}"
}

install_daemon_service() {
    AXON_BIN="$1"
    info "正在安装并启动 Daemon 用户服务（Remote/Feishu 配置可稍后完成）..."
    if [ -n "${CONFIG_PATH}" ]; then
        if "${AXON_BIN}" --config "${CONFIG_PATH}" daemon install; then
            success "Daemon 用户服务已安装并启动；未配置远控时仍可运行通用 Session/Cron 调度。"
        else
            die "Daemon 安装失败，请检查 Axon 配置路径或系统用户服务权限。"
        fi
    elif "${AXON_BIN}" daemon install; then
        success "Daemon 用户服务已安装并启动；未配置远控时仍可运行通用 Session/Cron 调度。"
    else
        die "Daemon 安装失败，请检查 Axon 配置路径或系统用户服务权限。"
    fi
}

remove_obsolete_releases() {
    [ -f "${CURRENT_FILE}" ] || return 0
    CURRENT_ID="$(sed -n 's/.*"versionId"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "${CURRENT_FILE}")"
    [ -n "${CURRENT_ID}" ] || return 0
    CURRENT_RELEASE="${VERSIONS_DIR}/${CURRENT_ID}"
    LEASE_DIR="${INSTALL_ROOT}/state/runtime-release-leases"
    for RELEASE in "${VERSIONS_DIR}"/*; do
        [ -d "${RELEASE}" ] || continue
        RELEASE_ID="$(basename "${RELEASE}")"
        case "${RELEASE_ID}" in
            .*) continue ;;
        esac
        [ "${RELEASE}" = "${CURRENT_RELEASE}" ] && continue
        PROTECTED=0
        if [ -d "${LEASE_DIR}" ]; then
            for LEASE in "${LEASE_DIR}"/*; do
                [ -f "${LEASE}" ] || continue
                LEASE_NAME="$(basename "${LEASE}")"
                LEASE_PID="${LEASE_NAME%.lease}"
                case "${LEASE_PID}" in
                    ''|*[!0-9]*)
                        rm -f "${LEASE}"
                        continue
                        ;;
                esac
                if ! kill -0 "${LEASE_PID}" 2>/dev/null; then
                    rm -f "${LEASE}"
                    continue
                fi
                if [ "$(cat "${LEASE}")" = "${RELEASE}" ]; then
                    PROTECTED=1
                    break
                fi
            done
        fi
        if [ "${PROTECTED}" -eq 0 ]; then
            rm -rf "${RELEASE}"
            info "已回收旧 Axon release：${RELEASE_ID}"
        fi
    done
}

# ── PATH 自动配置 ────────────────────────────────────────────────────────────

configure_path() {
    SHELL_NAME="$(basename "${SHELL:-sh}")"
    case "${SHELL_NAME}" in
        zsh) PROFILE="${ZDOTDIR:-${HOME}}/.zshrc" ;;
        fish) PROFILE="${HOME}/.config/fish/config.fish" ;;
        *) PROFILE="${HOME}/.bashrc" ;;
    esac
    mkdir -p "$(dirname "${PROFILE}")"
    if [ "${SHELL_NAME}" = "fish" ]; then
        PATH_LINE="fish_add_path \"${INSTALL_DIR}\" \"${LAUNCHER_DIR}\""
    else
        PATH_LINE="export PATH=\"${INSTALL_DIR}:${LAUNCHER_DIR}:\$PATH\""
    fi
    if ! grep -Fqx "${PATH_LINE}" "${PROFILE}" 2>/dev/null; then
        printf '\n%s\n' "${PATH_LINE}" >> "${PROFILE}"
    fi
    export PATH="${INSTALL_DIR}:${LAUNCHER_DIR}:${PATH}"
    success "CLI 与 launcher 目录已加入 PATH：${INSTALL_DIR}、${LAUNCHER_DIR}"
}

# ── 主流程 ────────────────────────────────────────────────────────────────────

main() {
    info "正在检测平台..."
    TARGET="$(detect_target)"
    info "目标平台：${TARGET}"

    info "正在获取最新版本..."
    VERSION="$(get_latest_version)"
    if [ -z "${VERSION}" ]; then
        die "无法获取最新版本号，请检查网络连接或访问 https://github.com/${REPO}/releases 手动下载。"
    fi
    info "最新版本：v${VERSION}"

    # 检查是否已安装最新版本
    if command -v axon >/dev/null 2>&1; then
        CURRENT="$(axon --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo '')"
        if [ "${CURRENT}" = "${VERSION}" ]; then
            if [ -f "${SKILLS_DIR}/${SKILL_NAME}/SKILL.md" ] && { [ -e "${SOUL_FILE}" ] || [ -L "${SOUL_FILE}" ]; }; then
                configure_path
                install_daemon_service "$(command -v axon)"
                success "Axon v${VERSION}、${SKILL_NAME} Skill 和 Soul 均已安装，无需更新。"
                exit 0
            fi
            info "Axon v${VERSION} 已是最新版本，将补装 ${SKILL_NAME} Skill..."
        elif [ -n "${CURRENT}" ]; then
            info "当前版本 v${CURRENT}，将升级到 v${VERSION}..."
        fi
    fi

    # 构造下载 URL
    ARCHIVE_NAME="axon-v${VERSION}-${TARGET}.tar.gz"
    BASE_URL="https://github.com/${REPO}/releases/download/v${VERSION}"
    ARCHIVE_URL="${BASE_URL}/${ARCHIVE_NAME}"
    SHA256_URL="${ARCHIVE_URL}.sha256"

    # 创建临时目录
    TMP_DIR="$(mktemp -d)"
    trap 'release_install_lock; rm -rf "${TMP_DIR}"' EXIT INT TERM
    acquire_install_lock

    info "正在下载 ${ARCHIVE_NAME}..."
    download_file "${ARCHIVE_URL}" "${TMP_DIR}/${ARCHIVE_NAME}"

    info "正在校验 SHA256..."
    download_file "${SHA256_URL}" "${TMP_DIR}/${ARCHIVE_NAME}.sha256"
    EXPECTED_SHA="$(cat "${TMP_DIR}/${ARCHIVE_NAME}.sha256")"
    check_sha256 "${TMP_DIR}/${ARCHIVE_NAME}" "${EXPECTED_SHA}"
    success "SHA256 校验通过。"

    info "正在解压..."
    tar xzf "${TMP_DIR}/${ARCHIVE_NAME}" -C "${TMP_DIR}"

    # 解析发行包根目录。后续只发布完整目录，绝不覆盖正在运行的版本文件。
    PACKAGE_ROOT="${TMP_DIR}/axon-v${VERSION}-${TARGET}"
    EXTRACTED_BINARY="${PACKAGE_ROOT}/${BINARY_NAME}"
    if [ ! -f "${EXTRACTED_BINARY}" ]; then
        # 兼容直接放在根目录的结构
        PACKAGE_ROOT="${TMP_DIR}"
        EXTRACTED_BINARY="${TMP_DIR}/${BINARY_NAME}"
    fi
    if [ ! -f "${EXTRACTED_BINARY}" ]; then
        die "解压后未找到 axon 二进制文件，请联系开发者。"
    fi
    if [ ! -f "${PACKAGE_ROOT}/launcher/axon-launcher" ]; then
        die "发行包未包含 axon-launcher，请联系开发者。"
    fi
    RELEASE_NAME="${VERSION}"
    RELEASE_DIR="${VERSIONS_DIR}/${RELEASE_NAME}"
    STAGING_DIR="${VERSIONS_DIR}/.staging.${RELEASE_NAME}.$$"
    mkdir -p "${VERSIONS_DIR}" "${INSTALL_DIR}" "${LAUNCHER_DIR}"
    if [ -x "${LAUNCHER_PATH}" ]; then
        "${LAUNCHER_PATH}" daemon stop >/dev/null 2>&1 || true
    fi
    pkill -x axon-desktop >/dev/null 2>&1 || true
    rm -rf "${STAGING_DIR}"
    mkdir -p "${STAGING_DIR}"
    cp -R "${PACKAGE_ROOT}/." "${STAGING_DIR}"
    chmod 755 "${STAGING_DIR}/${BINARY_NAME}"

    if ! "${STAGING_DIR}/${BINARY_NAME}" --version >/dev/null 2>&1; then
        die "staging 版本无法通过 axon --version 验证。"
    fi

    rm -rf "${RELEASE_DIR}"
    mv "${STAGING_DIR}" "${RELEASE_DIR}"

    # launcher 自身保持稳定，但每次安装都更新到本次发行版。
    cp "${PACKAGE_ROOT}/launcher/axon-launcher" "${LAUNCHER_PATH}"
    chmod 755 "${LAUNCHER_PATH}"

    # current.json 以 rename 发布；运行中的旧进程继续使用旧版本文件。
    CURRENT_NEXT="${INSTALL_ROOT}/.current.${VERSION}.$$.json"
    printf '{"versionId":"%s"}\n' "${RELEASE_NAME}" > "${CURRENT_NEXT}"
    mv -f "${CURRENT_NEXT}" "${CURRENT_FILE}"

    VISIBLE_NEXT="${INSTALL_DIR}/.${BINARY_NAME}.${VERSION}.$$"
    rm -f "${VISIBLE_NEXT}"
    ln -s "${LAUNCHER_PATH}" "${VISIBLE_NEXT}"
    mv -f "${VISIBLE_NEXT}" "${INSTALL_DIR}/${BINARY_NAME}"
    success "Axon v${VERSION} 已发布到 ${RELEASE_DIR}"

    install_bundled_skill "${RELEASE_DIR}"
    install_bundled_soul "${RELEASE_DIR}"
    initialize_owner_identity

    # 验证安装
    if "${LAUNCHER_PATH}" --version >/dev/null 2>&1; then
        success "安装验证通过：$("${LAUNCHER_PATH}" --version)"
    else
        warn "安装完成，但 axon --version 执行异常，请检查二进制文件。"
    fi

    install_daemon_service "${LAUNCHER_PATH}"
    remove_obsolete_releases

    configure_path

    echo ""
    success "安装完成！运行 'axon --help' 开始使用。"
    info "如需更新，重新运行本脚本或执行：axon upgrade"
}

main "$@"
