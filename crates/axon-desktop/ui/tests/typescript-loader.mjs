import { registerHooks } from 'node:module';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

// 使用项目已有编译器，让 Node 的测试执行真实 TypeScript 模块。
registerHooks({
    resolve(specifier, context, nextResolve) {
        if (specifier.startsWith('.') && context.parentURL) {
            for (const suffix of ['.ts', '.tsx', '/index.ts']) {
                const url = new URL(specifier + suffix, context.parentURL);
                if (existsSync(fileURLToPath(url))) return { url: url.href, shortCircuit: true };
            }
        }
        return nextResolve(specifier, context);
    },
    load(url, context, nextLoad) {
        if (/\.tsx?$/.test(url)) {
            const source = readFileSync(fileURLToPath(url), 'utf8');
            return {
                format: 'module', shortCircuit: true,
                source: ts.transpileModule(source, {
                    compilerOptions: {
                        module: ts.ModuleKind.ESNext,
                        target: ts.ScriptTarget.ESNext,
                        // 界面组件以 .tsx 落地：这里按 React 17 的自动运行时转译 JSX，测试才能直接渲染组件。
                        jsx: ts.JsxEmit.ReactJSX,
                    },
                }).outputText,
            };
        }
        return nextLoad(url, context);
    },
});
