import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App';
import { TextSelectionMenuHost } from './components/ui/TextSelectionMenuHost';
import './tokens.css';

/*
 * 拖入文件时 webview 默认会导航到那个文件，界面直接被替换掉。
 *
 * 窗口级一律拦下默认行为，真正的落点只有输入坞自己（它会在 drop 时接管并落地附件）。
 * 这条守卫在输入坞的处理器之后运行（事件从落点向上冒泡），因此不会抢走它的处理。
 *
 * 右键菜单不在这里处理：它由 TextSelectionMenuHost 统一负责。那里既压掉 webview
 * 自带的菜单，也给出全站一致的那一份（只读区域只有复制，可编辑区域用输入框菜单）。
 */
document.addEventListener('dragover', (event) => event.preventDefault());
document.addEventListener('drop', (event) => event.preventDefault());

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
    <TextSelectionMenuHost />
  </React.StrictMode>
);
