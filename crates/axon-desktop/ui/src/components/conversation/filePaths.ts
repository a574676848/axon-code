/**
 * 正文只识别明确的本地路径，避免把性能分位数与 I/O 当成文件。
 *
 * 判定要求「路径形状」而不是「结尾像扩展名」：裸词 `C`、`go`、`ts`、`rs`、`h` 都是
 * 扩展名本身，正文里常作普通词使用（「方案 C」「用 Go 重写」），只看后缀会把它们
 * 渲染成文件芯片。
 */
const FILE_EXTENSIONS = new Set([
    'rs', 'ts', 'tsx', 'js', 'jsx', 'json', 'md', 'toml', 'yaml', 'yml', 'py',
    'go', 'java', 'kt', 'css', 'html', 'sh', 'ps1', 'sql', 'lock', 'txt',
    'csv', 'xml', 'svg', 'png', 'jpg', 'jpeg', 'pdf', 'vue', 'svelte', 'c', 'h',
    'cpp', 'hpp', 'cs', 'rb', 'php', 'ini', 'conf', 'log', 'jsonl',
]);
const FILE_NAMES = new Set(['Dockerfile', 'Makefile', '.env', '.gitignore']);
const PATH_TOKEN = /(?:https?:\/\/|www\.)[^\s<>"`]+|(?:[A-Za-z]:[\\/]|\\\\|\.{1,2}[\\/]|\/)?[\p{L}\p{N}_@.~-]+(?:[\\/][\p{L}\p{N}_@~.-]+)*/gu;

/** 明确的路径形状：盘符、UNC、以 / ~/ ./ ../ 开头。 */
const EXPLICIT_PATH = /^(?:[A-Za-z]:[\\/]|\\\\|~?[\\/]|\.{1,2}[\\/])/;

/** 末段是否为「名字.后缀」，且后缀在已知清单里。 */
function hasKnownExtension(filename: string): boolean {
    const dot = filename.lastIndexOf('.');
    if (dot <= 0 || dot === filename.length - 1) return false;
    return FILE_EXTENSIONS.has(filename.slice(dot + 1).toLowerCase());
}

export function isFilePath(value: string): boolean {
    if (/^(?:https?:\/\/|www\.)/i.test(value)) return false;
    if (EXPLICIT_PATH.test(value)) return true;
    const segments = value.split(/[\\/]/);
    const filename = segments[segments.length - 1] ?? '';
    return FILE_NAMES.has(filename) || hasKnownExtension(filename);
}

export function filePathsIn(text: string): { path: string; index: number }[] {
    return Array.from(text.matchAll(PATH_TOKEN))
        .map((match) => ({ path: match[0].replace(/\.+$/, ''), index: match.index }))
        .filter(({ path }) => isFilePath(path));
}
