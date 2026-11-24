import { useEffect } from 'react';

const APP_TITLE = 'axon';

/** 原生标题栏与浏览器标签同步当前会话名称。 */
export function useWindowTitle(conversationTitle: string | undefined): void {
    const title = conversationTitle?.trim() || APP_TITLE;
    useEffect(() => {
        let active = true;
        document.title = title;
        if ('__TAURI_INTERNALS__' in window) {
            void import('@tauri-apps/api/window').then(({ getCurrentWindow }) => {
                if (active) return getCurrentWindow().setTitle(title);
            }).catch((failure: unknown) => {
                console.error('更新窗口标题失败', failure);
            });
        }
        return () => { active = false; };
    }, [title]);
}
