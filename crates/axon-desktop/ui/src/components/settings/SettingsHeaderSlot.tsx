import React, { createContext, useContext } from 'react';
import { createPortal } from 'react-dom';

/*
 * 分区标题行的两个插槽。
 *
 * 标题行归设置弹窗所有（标题、说明、与内容右轴对齐的动作区），
 * 而「添加服务器」「新增定价」「保存修改」这类动作的状态住在各自分区里。
 * 通过插槽让分区把动作挂到标题行上，既不用把状态提升到弹窗外层，
 * 也不会再在正文里单独起一行按钮。
 */

const HeaderActionContext = createContext<HTMLElement | null>(null);
const HeaderStatusContext = createContext<HTMLElement | null>(null);

export interface SettingsHeaderProviderProps {
  /** 标题行右侧的动作插槽。 */
  actionSlot: HTMLElement | null;
  /** 说明行右端的状态插槽。 */
  statusSlot: HTMLElement | null;
  children: React.ReactNode;
}

export const SettingsHeaderProvider: React.FC<SettingsHeaderProviderProps> = ({
  actionSlot,
  statusSlot,
  children,
}) => (
  <HeaderActionContext.Provider value={actionSlot}>
    <HeaderStatusContext.Provider value={statusSlot}>{children}</HeaderStatusContext.Provider>
  </HeaderActionContext.Provider>
);

/** 把分区内的主操作挂到标题行右端；插槽尚未挂载时不渲染。 */
export const SettingsHeaderAction: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const slot = useContext(HeaderActionContext);
  return slot === null ? null : createPortal(children, slot);
};

/** 把分区内的内联状态挂到说明行右端，例如文档的「已保存」。 */
export const SettingsHeaderStatus: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const slot = useContext(HeaderStatusContext);
  return slot === null ? null : createPortal(children, slot);
};
