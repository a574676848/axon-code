#!/usr/bin/env bash
# 安装 Axon 语音所需的第三方运行时与模型（sherpa-onnx + VAD/ASR/TTS）。
# 由安装脚本自动调用；已安装且版本一致时直接跳过。
set -euo pipefail

RUNTIME_ROOT="${AXON_RUNTIME_ROOT:-${HOME}/.axon}"
SHERPA_VERSION="2.0.1"
RUNTIME_DIR="${RUNTIME_ROOT}/voice/runtime/sherpa-onnx/${SHERPA_VERSION}"
MODELS_DIR="${RUNTIME_ROOT}/voice/models"
DOWNLOADS="${RUNTIME_ROOT}/voice/downloads"
MARKER="${RUNTIME_ROOT}/voice/.provisioned.json"

case "$(uname -s)" in
    Linux) PLATFORM="linux-x64" ;;
    Darwin)
        if [ "$(uname -m)" = "arm64" ]; then PLATFORM="osx-arm64"; else PLATFORM="osx-x64"; fi
        ;;
    *) echo "不支持的平台：$(uname -s)" >&2; exit 1 ;;
esac

RUNTIME_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/v${SHERPA_VERSION}/sherpa-onnx-v${SHERPA_VERSION}-${PLATFORM}-shared.tar.bz2"
VAD_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx"
ASR_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20.tar.bz2"
TTS_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-zh-hf-fanchen-C.tar.bz2"

fetch() {
    local url="$1"
    mkdir -p "${DOWNLOADS}"
    local file="${DOWNLOADS}/$(basename "${url}")"
    if [ ! -s "${file}" ]; then
        echo "[voice] 下载 $(basename "${url}")"
        curl -L --fail --retry 3 --silent --show-error -o "${file}" "${url}"
    fi
    printf '%s\n' "${file}"
}

manifest() {
    local family="$1" name="$2" dir="$3" languages="$4" rtf50="$5" rtf95="$6" ram="$7" license="$8"
    local files_json="" first=1 path hash bytes
    while IFS= read -r file; do
        path="${file#${dir}/}"
        hash="$(sha256sum "${file}" | awk '{print $1}')"
        bytes="$(wc -c < "${file}" | tr -d ' ')"
        if [ "${first}" -eq 0 ]; then files_json="${files_json},"; fi
        files_json="${files_json}{\"path\":\"${path}\",\"sha256\":\"${hash}\",\"bytes\":${bytes}}"
        first=0
    done < <(find "${dir}" -type f ! -name manifest.json | sort)

    cat > "${dir}/manifest.json" <<EOF
{
  "family": "${family}",
  "name": "${name}",
  "version": "1.0.0",
  "runtime": "sherpa-onnx",
  "sampleRate": 16000,
  "languages": [${languages}],
  "license": "${license}",
  "rtfP50": ${rtf50},
  "rtfP95": ${rtf95},
  "ramMb": ${ram},
  "files": [${files_json}]
}
EOF
}

if [ "${1:-}" != "--force" ] && [ -f "${MARKER}" ] && grep -q "\"sherpaVersion\": \"${SHERPA_VERSION}\"" "${MARKER}"; then
    echo "[voice] 运行时与模型已安装（${SHERPA_VERSION}），跳过。"
    exit 0
fi

mkdir -p "${RUNTIME_DIR}" "${MODELS_DIR}"

if [ ! -x "${RUNTIME_DIR}/bin/sherpa-onnx" ]; then
    archive="$(fetch "${RUNTIME_URL}")"
    staging="${RUNTIME_ROOT}/voice/runtime/.staging"
    rm -rf "${staging}"
    mkdir -p "${staging}"
    tar -xjf "${archive}" -C "${staging}"
    inner="$(find "${staging}" -mindepth 1 -maxdepth 1 -type d | head -n1)"
    rm -rf "${RUNTIME_DIR}"
    mv "${inner}" "${RUNTIME_DIR}"
    rm -rf "${staging}"
    chmod +x "${RUNTIME_DIR}/bin"/* 2>/dev/null || true
fi

install_model() {
    local family="$1" name="$2" archive="$3" inner_hint="$4"
    local target="${MODELS_DIR}/${family}/${name}"
    if [ -f "${target}/manifest.json" ]; then return 0; fi
    local staging="${RUNTIME_ROOT}/voice/models/.staging-${name}"
    rm -rf "${staging}"
    mkdir -p "${staging}"
    case "${archive}" in
        *.onnx) cp "${archive}" "${staging}/silero_vad.onnx" ;;
        *)
            tar -xjf "${archive}" -C "${staging}"
            local inner
            inner="$(find "${staging}" -mindepth 1 -maxdepth 1 -type d | head -n1)"
            if [ -z "${inner}" ]; then inner="${staging}"; fi
            find "${inner}" -mindepth 1 -maxdepth 1 -exec mv {} "${staging}/" \;
            rmdir "${inner}" 2>/dev/null || true
            ;;
    esac
    rm -rf "${target}"
    mkdir -p "$(dirname "${target}")"
    mv "${staging}" "${target}"
    echo "${inner_hint}"
}

vad_archive="$(fetch "${VAD_URL}")"
install_model vad silero-vad-v4 "${vad_archive}" >/dev/null
manifest vad silero-vad-v4 "${MODELS_DIR}/vad/silero-vad-v4" '"zh"' 0.01 0.02 50 MIT

asr_archive="$(fetch "${ASR_URL}")"
install_model asr sherpa-streaming-zh-en "${asr_archive}" >/dev/null
manifest asr sherpa-streaming-zh-en "${MODELS_DIR}/asr/sherpa-streaming-zh-en" '"zh","en"' 0.25 0.45 420 Apache-2.0

tts_archive="$(fetch "${TTS_URL}")"
install_model tts vits-zh-fanchen "${tts_archive}" >/dev/null
manifest tts vits-zh-fanchen "${MODELS_DIR}/tts/vits-zh-fanchen" '"zh"' 0.20 0.40 300 Apache-2.0

printf '{"sherpaVersion":"%s","models":3,"installedAt":"%s"}\n' "${SHERPA_VERSION}" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "${MARKER}"
echo "[voice] 语音运行时与模型就绪：${RUNTIME_ROOT}/voice"
