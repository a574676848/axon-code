#!/usr/bin/env bash
set -euo pipefail

# 编译期 lint 豁免检查的入口。
#
# 这里**只负责选定解释器**，检测逻辑在 check_no_lint_allows.py 中，单点实现。
#
# 为什么不再直接用 ripgrep：
#   旧实现以 `rg ... || true` 收集结果。ripgrep 缺失时命令替换结果为空，
#   脚本照常打印「no compile-time lint suppressions found」并以 exit 0 结束 ——
#   与「扫描后无命中」产出**逐字相同**的输出，守卫因此退化为假保证。
#   实测：本仓开发机（Windows 与 WSL Ubuntu-24.04）都没有 ripgrep，该守卫
#   从未真正执行过；而 windows-latest runner 同样不预装 ripgrep
#   （其镜像构建目录 images/windows/scripts/build 的 88 个安装脚本中
#   没有任何 ripgrep 安装项）。
#
# 为什么改用 Python 3：
#   它已是本仓验证链的既有前提（tools/verify-architecture.ps1 在缺少 Python 时
#   直接失败），在 ubuntu / macOS / Windows 三平台 runner 与开发机上均可用，
#   因此无须新增任何安装步骤或 CI 网络依赖。

# 只使用 bash 内建（`cd` / `pwd`）推导脚本目录，不依赖外部 `dirname`：
# 否则 PATH 异常时会在自检之前就以一句难懂的错误失败。
script_path="${BASH_SOURCE[0]}"
[[ "${script_path}" == */* ]] || script_path="./${script_path}"
script_dir="$(cd -- "${script_path%/*}" && pwd)"

# 逐个探测候选解释器，只接受**确实能作为 Python 3 运行**的那一个。
# 不能只判断命令是否存在：Windows 上 `python3` 可能是 Microsoft Store 的占位程序，
# 存在却无法执行，会让「找到解释器」成为假象。
interpreter=""
for candidate in python3 python py; do
  if ! command -v "${candidate}" >/dev/null 2>&1; then
    continue
  fi
  if "${candidate}" -c 'import sys; raise SystemExit(0 if sys.version_info[0] == 3 else 1)' >/dev/null 2>&1; then
    interpreter="${candidate}"
    break
  fi
done

if [[ -z "${interpreter}" ]]; then
  printf '%s\n' \
    "错误：本守卫需要 Python 3，但未找到可用解释器（已尝试 python3 / python / py）。" \
    "安装： apt-get install python3  |  brew install python3  |  winget install Python.Python.3" \
    "守卫必须 fail-closed：宁可显式失败，也不得给出未经验证的「已通过」结论。" >&2
  exit 1
fi

exec "${interpreter}" -X utf8 "${script_dir}/check_no_lint_allows.py"
