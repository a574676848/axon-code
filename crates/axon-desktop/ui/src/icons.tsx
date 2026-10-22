import React from 'react';

/*
 * 图标集合。
 *
 * 除品牌标识 AxonLogo 外，全部图标路径按值取自已有的工作台产品图标集，
 * 统一 16 视窗、1px 描边、圆头圆角，禁止在此手绘近似图形。
 * 按值拷贝而非引入依赖，保证桌面端离线自足。
 */

export interface IconProps extends React.SVGProps<SVGSVGElement> {
  /** 边长像素。 */
  size?: number;
  /** 附加类名。 */
  className?: string;
}

/*
 * 取色一律走设计令牌，图标文件里不出现颜色字面量。
 * 用内联 style 而不是 SVG 的呈现属性：呈现属性上的 var() 在部分引擎不生效，
 * 内联样式则是标准 CSS 声明，任何支持自定义属性的引擎都能解析。
 */
const brandFill = (name: string): React.CSSProperties => ({ fill: `var(${name})` });
const brandStroke = (name: string): React.CSSProperties => ({ stopColor: `var(${name})` });

/** 品牌标识：项目真实标识的矢量还原。 */
export const AxonLogo: React.FC<IconProps> = ({ size = 20, className = '', ...props }) => {
  // 渐变需要唯一 id，避免同页多份标识互相覆盖。
  const gradientId = `axon-logo-${React.useId().replaceAll(':', '')}`;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 200 200"
      role="img"
      aria-label="Axon"
      className={className}
      {...props}
    >
      <defs>
        <linearGradient id={gradientId} x1="0%" y1="0%" x2="100%" y2="100%">
          <stop offset="0%" style={brandStroke('--color-brand-start')} />
          <stop offset="50%" style={brandStroke('--color-brand-middle')} />
          <stop offset="100%" style={brandStroke('--color-brand-end')} />
        </linearGradient>
      </defs>
      <rect x="10" y="10" width="180" height="180" rx="20" style={brandFill('--color-brand-plate')} opacity="0.95" />
      <rect
        x="15"
        y="15"
        width="170"
        height="170"
        rx="15"
        fill="none"
        stroke={`url(#${gradientId})`}
        strokeWidth="2"
        opacity="0.3"
      />
      {/* 神经元轴突：树突分支、轴突交叉与突触节点 */}
      <g transform="translate(100, 100)" stroke={`url(#${gradientId})`} strokeLinecap="round" fill="none">
        <path d="M -40 0 L -25 -20 M -40 0 L -25 20" strokeWidth="3" />
        <path
          d="M -15 -20 L 0 0 L -15 20 M -15 -20 L 0 0 L 15 -20 M -15 20 L 0 0 M 0 0 L 15 20"
          strokeWidth="3.5"
        />
        <path d="M 25 -20 L 40 0 M 25 20 L 40 0" strokeWidth="3" />
        <circle cx="-40" cy="0" r="3" style={brandFill('--color-brand-start')} stroke="none" />
        <circle cx="-25" cy="-20" r="2.5" style={brandFill('--color-brand-middle')} stroke="none" />
        <circle cx="-25" cy="20" r="2.5" style={brandFill('--color-brand-middle')} stroke="none" />
        <circle cx="0" cy="0" r="4" style={brandFill('--color-brand-end')} stroke="none" />
        <circle cx="25" cy="-20" r="2.5" style={brandFill('--color-brand-middle')} stroke="none" />
        <circle cx="25" cy="20" r="2.5" style={brandFill('--color-brand-middle')} stroke="none" />
        <circle cx="40" cy="0" r="3" style={brandFill('--color-brand-start')} stroke="none" />
      </g>
    </svg>
  );
};

// 新会话：对话气泡加号
export const IconNewChat: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={className} {...props}>
    <path d="M2.37091 11.2501C1.58745 9.89288 1.32067 8.29835 1.61969 6.76006C1.91872 5.22177 2.76342 3.8433 3.99826 2.87846C5.2331 1.91362 6.77494 1.42737 8.33988 1.50925C9.90482 1.59113 11.3875 2.23562 12.5149 3.32406C13.6425 4.41269 14.3387 5.87206 14.4754 7.4334C14.612 8.99474 14.18 10.5529 13.2587 11.8209C12.3375 13.0888 10.9891 13.9813 9.46194 14.3337C8.18691 14.628 6.85895 14.5294 5.64989 14.0605C5.1712 13.8748 4.76962 13.4932 4.26534 13.3967C3.67413 13.2835 2.95257 13.5598 2.03794 14.3337" stroke="currentColor" />
    <path d="M8 5V11" stroke="currentColor" />
    <path d="M5 8H11" stroke="currentColor" />
  </svg>
);

// 侧栏折叠
export const IconPanelLeft: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M13.5 1.5H2.5C1.94772 1.5 1.5 1.94772 1.5 2.5V13.5C1.5 14.0523 1.94772 14.5 2.5 14.5H13.5C14.0523 14.5 14.5 14.0523 14.5 13.5V2.5C14.5 1.94772 14.0523 1.5 13.5 1.5Z" stroke="currentColor" />
    <path d="M5.5 1.5V14.5" stroke="currentColor" />
  </svg>
);

// 搜索
// 搜索：与既有工作台产品同款路径
export const SearchIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M6.58727 11.8586C9.55061 11.8586 11.9529 9.45637 11.9529 6.49304C11.9529 3.5297 9.55061 1.12744 6.58727 1.12744C3.62394 1.12744 1.22168 3.5297 1.22168 6.49304C1.22168 9.45637 3.62394 11.8586 6.58727 11.8586Z" stroke="currentColor" />
    <path d="M10.2991 10.3933L14.7783 14.8725" stroke="currentColor" />
  </svg>
);
// 视图选项：双滑块
export const SlidersIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1" strokeLinecap="round" aria-hidden="true" className={className} {...props}>
    <path d="M2.3 5h5.85M12.05 5h1.65" />
    <circle cx="9.95" cy="5" r="1.45" />
    <path d="M2.3 11h1.65M7.85 11h5.85" />
    <circle cx="5.75" cy="11" r="1.45" />
  </svg>
);

// 用量统计：条形图，与设置分区里其余线性图标同款一像素描边
export const ChartIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M2.5 2V13.5H14" stroke="currentColor" strokeLinecap="square" />
    <path d="M5.5 13.5V8.75" stroke="currentColor" strokeLinecap="square" />
    <path d="M8.5 13.5V4.5" stroke="currentColor" strokeLinecap="square" />
    <path d="M11.5 13.5V10.5" stroke="currentColor" strokeLinecap="square" />
  </svg>
);

// 模型定价：价签
export const PriceTagIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M7.9 1.9H13.1C13.65 1.9 14.1 2.35 14.1 2.9V8.1C14.1 8.37 14.0 8.63 13.81 8.82L8.82 13.81C8.43 14.2 7.8 14.2 7.41 13.81L2.19 8.59C1.8 8.2 1.8 7.57 2.19 7.18L7.18 2.19C7.37 2.0 7.63 1.9 7.9 1.9Z" stroke="currentColor" />
    <path d="M10.9 5.9C11.35 5.9 11.72 5.53 11.72 5.08C11.72 4.63 11.35 4.26 10.9 4.26C10.45 4.26 10.08 4.63 10.08 5.08C10.08 5.53 10.45 5.9 10.9 5.9Z" stroke="currentColor" />
  </svg>
);

// 工作区：合拢的文件夹
export const FolderIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M1.50439 3.11059C1.50439 2.55831 1.95211 2.1106 2.50439 2.1106H5.43389C5.67773 2.1106 5.91318 2.19969 6.09593 2.36113L7.71649 3.79265C7.89924 3.95409 8.1347 4.04319 8.3785 4.04319H13.4958C14.0481 4.04319 14.4958 4.4909 14.4958 5.04319V12.8894C14.4958 13.4417 14.0481 13.8894 13.4958 13.8894H2.50439C1.95211 13.8894 1.50439 13.4417 1.50439 12.8894V4.04319V3.11059Z" stroke="currentColor" />
    <path d="M3.63501 7.66614H12.3647" stroke="currentColor" />
  </svg>
);

// 工作区：展开的文件夹，与既有工作台产品同款的线性轮廓
export const FolderOpenIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M12.3994 13.5986H2.04956C1.49728 13.5986 1.04956 13.1509 1.04956 12.5986V3.40137C1.04956 2.84908 1.49728 2.40137 2.04956 2.40137H4.76632C5.01016 2.40137 5.24561 2.49046 5.42836 2.6519L6.94088 3.98799C7.12364 4.14943 7.35908 4.23852 7.60293 4.23852H12.3994C12.9517 4.23852 13.3994 4.68624 13.3994 5.23852V7.16991" stroke="currentColor" />
    <path d="M2.55911 7.93683C2.67584 7.49906 3.07229 7.19446 3.52536 7.19446H13.6491C14.3061 7.19446 14.7846 7.81725 14.6153 8.45209L13.4411 12.856C13.3244 13.2938 12.9279 13.5984 12.4748 13.5984H2.35113C1.69411 13.5984 1.21562 12.9756 1.38489 12.3407L2.55911 7.93683Z" stroke="currentColor" />
  </svg>
);

// 设置：齿轮
// 设置：与既有工作台产品同款路径
export const SettingsIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M8 9.75012C8.9665 9.75012 9.75 8.96662 9.75 8.00012C9.75 7.03362 8.9665 6.25012 8 6.25012C7.0335 6.25012 6.25 7.03362 6.25 8.00012C6.25 8.96662 7.0335 9.75012 8 9.75012Z" stroke="currentColor" />
    <path d="M13.0107 7.79377C12.9505 7.89401 12.9205 7.94413 12.9205 7.99951C12.9205 8.0549 12.9505 8.10502 13.0106 8.20528L13.9849 9.83006C14.045 9.93029 14.0751 9.9804 14.0751 10.0358C14.0751 10.0911 14.045 10.1413 13.9849 10.2415L13.0037 11.8777C12.9468 11.9726 12.9184 12.0201 12.8725 12.0461C12.8267 12.072 12.7713 12.072 12.6607 12.072H10.6704C10.5598 12.072 10.5045 12.072 10.4586 12.098C10.4128 12.1239 10.3843 12.1714 10.3274 12.2662L9.33825 13.9142C9.28133 14.009 9.25287 14.0564 9.20703 14.0823C9.16118 14.1083 9.10588 14.1083 8.99529 14.1083H7.00486C6.89426 14.1083 6.83896 14.1083 6.79312 14.0823C6.74727 14.0564 6.71881 14.009 6.6619 13.9142L5.67273 12.2662C5.61581 12.1714 5.58735 12.1239 5.54151 12.098C5.49566 12.072 5.44036 12.072 5.32977 12.072H3.33945C3.2288 12.072 3.17347 12.072 3.12761 12.0461C3.08176 12.0201 3.0533 11.9726 2.9964 11.8777L2.0152 10.2415C1.9551 10.1413 1.92505 10.0911 1.92505 10.0358C1.92505 9.9804 1.9551 9.93029 2.0152 9.83006L2.98951 8.20528C3.04963 8.10502 3.07969 8.0549 3.07969 7.99951C3.07968 7.94413 3.04961 7.89401 2.98946 7.79377L2.01529 6.17011C1.95514 6.06987 1.92507 6.01975 1.92507 5.96437C1.92506 5.90899 1.95512 5.85886 2.01524 5.7586L2.9964 4.1224C3.0533 4.0275 3.08176 3.98005 3.12761 3.95408C3.17347 3.92811 3.2288 3.92811 3.33945 3.92811H5.32977C5.44036 3.92811 5.49566 3.92811 5.54151 3.90216C5.58735 3.87621 5.61581 3.82879 5.67273 3.73397L6.6619 2.08599C6.71881 1.99116 6.74727 1.94375 6.79312 1.9178C6.83896 1.89185 6.89426 1.89185 7.00486 1.89185H8.99529C9.10588 1.89185 9.16118 1.89185 9.20703 1.9178C9.25287 1.94375 9.28133 1.99116 9.33825 2.08599L10.3274 3.73397C10.3843 3.82879 10.4128 3.87621 10.4586 3.90216C10.5045 3.92811 10.5598 3.92811 10.6704 3.92811H12.6607C12.7713 3.92811 12.8267 3.92811 12.8725 3.95408C12.9184 3.98005 12.9468 4.0275 13.0037 4.1224L13.9849 5.7586C14.045 5.85886 14.0751 5.90899 14.0751 5.96437C14.0751 6.01975 14.045 6.06987 13.9849 6.17011L13.0107 7.79377Z" stroke="currentColor" />
  </svg>
);

// 密钥
export const KeyIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M6.80132 2.14853C7.70663 1.80917 8.70422 1.80919 9.60952 2.14859L14.1296 3.84317V7.11961C14.1296 11.6089 10.7615 13.5975 8.20543 14.5779C5.64931 13.5975 2.28052 11.6089 2.28052 7.11961V3.84317L6.80132 2.14853Z" stroke="currentColor" strokeLinejoin="round" />
  </svg>
);

// 置顶：图钉
// 固定：与既有工作台产品同款路径
export const PinIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M9.96976 1.70572L13.1554 3.93629L10.9019 8.12317L11.5158 11.605L10.7192 12.7427L2.52767 7.00693L3.3243 5.86922L6.80612 5.25528L9.96976 1.70572Z" stroke="currentColor" strokeLinejoin="round" />
    <path d="M6.05285 9.47511C6.27284 9.16094 6.70586 9.08458 7.02003 9.30457C7.3342 9.52455 7.41055 9.95757 7.19057 10.2717L3.98587 14.4708L3.21223 13.9291L6.05285 9.47511Z" fill="currentColor" />
  </svg>
);

// 已置顶：实心图钉，路径与描边版同源，只把针身填满
export const PinFillIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M9.96976 1.70572L13.1554 3.93629L10.9019 8.12317L11.5158 11.605L10.7192 12.7427L2.52767 7.00693L3.3243 5.86922L6.80612 5.25528L9.96976 1.70572Z" fill="currentColor" stroke="currentColor" strokeLinejoin="round" />
    <path d="M6.05285 9.47511C6.27284 9.16094 6.70586 9.08458 7.02003 9.30457C7.3342 9.52455 7.41055 9.95757 7.19057 10.2717L3.98587 14.4708L3.21223 13.9291L6.05285 9.47511Z" fill="currentColor" />
  </svg>
);

// 重命名：铅笔
// 编辑：与既有工作台产品同款路径
export const EditIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M8.85596 2.69971H4.19971C3.37141 2.69971 2.69992 3.37146 2.69971 4.19971V11.8003C2.69992 12.6285 3.37141 13.3003 4.19971 13.3003H11.8003C12.6283 13.2999 13.3001 12.6283 13.3003 11.8003V7.89893H14.3003V11.8003C14.3001 13.1806 13.1806 14.2999 11.8003 14.3003H4.19971C2.81913 14.3003 1.69992 13.1808 1.69971 11.8003V4.19971C1.69992 2.81918 2.81913 1.69971 4.19971 1.69971H8.85596V2.69971Z" fill="currentColor" />
    <path d="M7.7849 8.23878L13.888 2.13574" stroke="currentColor" />
  </svg>
);

// 归档：收纳箱
// 归档：与既有工作台产品同款路径
export const ArchiveIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M13.5 2.5H2.5C1.94772 2.5 1.5 2.94772 1.5 3.5V4.5C1.5 5.05228 1.94772 5.5 2.5 5.5H13.5C14.0523 5.5 14.5 5.05228 14.5 4.5V3.5C14.5 2.94772 14.0523 2.5 13.5 2.5Z" stroke="currentColor" />
    <path d="M2.5 5.5V13.5C2.5 13.7652 2.60536 14.0196 2.79289 14.2071C2.98043 14.3946 3.23478 14.5 3.5 14.5H12.5C12.7652 14.5 13.0196 14.3946 13.2071 14.2071C13.3946 14.0196 13.5 13.7652 13.5 13.5V5.5" stroke="currentColor" />
    <path d="M6.5 9.5H9.5" stroke="currentColor" />
  </svg>
);

// 删除：垃圾桶
// 删除：与既有工作台产品同款路径
export const TrashIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M1.28149 3.88831H14.7187" stroke="currentColor" />
    <path d="M5.41602 3.88833V2.47962C5.41602 2.29282 5.52492 2.11366 5.71876 1.98157C5.9126 1.84948 6.17551 1.77527 6.44964 1.77527H9.55053C9.82466 1.77527 10.0876 1.84948 10.2814 1.98157C10.4753 2.11366 10.5842 2.29282 10.5842 2.47962V3.88833" stroke="currentColor" />
    <path d="M2.57349 3.88831L3.19366 13.2943C3.21937 13.5502 3.33952 13.7872 3.53065 13.9593C3.72178 14.1313 3.97016 14.2259 4.22729 14.2246H11.7728C12.0299 14.2259 12.2783 14.1313 12.4694 13.9593C12.6605 13.7872 12.7807 13.5502 12.8064 13.2943L13.4266 3.88831" stroke="currentColor" />
    <path d="M6.44946 6.98926V11.1238" stroke="currentColor" />
    <path d="M9.55054 6.98926V11.1238" stroke="currentColor" />
  </svg>
);

// 更多操作：省略号
export const MoreIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M3 9C3.55228 9 4 8.55228 4 8C4 7.44772 3.55228 7 3 7C2.44772 7 2 7.44772 2 8C2 8.55228 2.44772 9 3 9Z" fill="currentColor" />
    <path d="M8 9C8.55228 9 9 8.55228 9 8C9 7.44772 8.55228 7 8 7C7.44772 7 7 7.44772 7 8C7 8.55228 7.44772 9 8 9Z" fill="currentColor" />
    <path d="M13 9C13.5523 9 14 8.55228 14 8C14 7.44772 13.5523 7 13 7C12.4477 7 12 7.44772 12 8C12 8.55228 12.4477 9 13 9Z" fill="currentColor" />
  </svg>
);

// 进行中：环形缺口
export const SpinnerIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M12.596 12.596C11.687 13.5049 10.5288 14.1239 9.26798 14.3747C8.00716 14.6255 6.70028 14.4968 5.51261 14.0048C4.32494 13.5129 3.30981 12.6798 2.59557 11.611C1.88134 10.5421 1.50008 9.2855 1.5 7.99998C1.50008 6.71446 1.88134 5.45783 2.59557 4.38898C3.30981 3.32013 4.32494 2.48707 5.51261 1.99513C6.70028 1.50319 8.00716 1.37447 9.26798 1.62524C10.5288 1.87602 11.687 2.49502 12.596 3.40398" stroke="currentColor" />
  </svg>
);

// 对勾
// 勾选：与既有工作台产品同款路径
export const CheckIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M2.25 8.5L5.49732 11.7473C5.90519 12.1552 6.57263 12.1344 6.95426 11.7018L13.75 4" stroke="currentColor" />
  </svg>
);

// 警告
export const AlertCircleIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M8 14.5C11.5899 14.5 14.5 11.5899 14.5 8C14.5 4.41015 11.5899 1.5 8 1.5C4.41015 1.5 1.5 4.41015 1.5 8C1.5 11.5899 4.41015 14.5 8 14.5Z" stroke="currentColor" />
    <path d="M8 4.29199V9.79199" stroke="currentColor" />
    <path d="M8 10.708V11.708" stroke="currentColor" />
  </svg>
);

// 关闭：与既有工作台产品同款的两笔细线
export const CloseIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M3.5 3.5L12.5 12.5" stroke="currentColor" />
    <path d="M12.5 3.5L3.5 12.5" stroke="currentColor" />
  </svg>
);

// 下拉箭头
export const ChevronDownIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M4 6L7.29289 9.29289C7.68342 9.68342 8.31658 9.68342 8.70711 9.29289L12 6" stroke="currentColor" />
  </svg>
);

// 加号
export const PlusIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M8 2V14" stroke="currentColor" />
    <path d="M2 8H14" stroke="currentColor" />
  </svg>
);

// 远控：地球
export const GlobeIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M7.99986 14.0887C11.3626 14.0887 14.0886 11.3627 14.0886 7.99998C14.0886 4.63727 11.3626 1.91125 7.99986 1.91125C4.63715 1.91125 1.91113 4.63727 1.91113 7.99998C1.91113 11.3627 4.63715 14.0887 7.99986 14.0887Z" stroke="currentColor" />
    <path d="M2.34619 8H13.6538" stroke="currentColor" strokeLinecap="square" />
    <path d="M7.99976 14.0889C9.23509 14.0889 10.1743 11.3629 10.1743 8.00006C10.1743 4.63739 9.23509 1.91138 7.99976 1.91138" stroke="currentColor" />
    <path d="M7.99973 14.0889C6.76445 14.0889 5.8252 11.3629 5.8252 8.00006C5.8252 4.63739 6.76445 1.91138 7.99973 1.91138" stroke="currentColor" />
  </svg>
);

// 模型与接口
export const ApiIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M3 4L7 8L3 12" stroke="currentColor" />
    <path d="M9 12H13" stroke="currentColor" />
  </svg>
);

// 记忆与清单
export const ChecklistIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M3.75 6.25C4.7165 6.25 5.5 5.4665 5.5 4.5C5.5 3.5335 4.7165 2.75 3.75 2.75C2.7835 2.75 2 3.5335 2 4.5C2 5.4665 2.7835 6.25 3.75 6.25Z" stroke="currentColor" />
    <path d="M7.5 4.5H13.5" stroke="currentColor" />
    <path d="M3.75 13.25C4.7165 13.25 5.5 12.4665 5.5 11.5C5.5 10.5335 4.7165 9.75 3.75 9.75C2.7835 9.75 2 10.5335 2 11.5C2 12.4665 2.7835 13.25 3.75 13.25Z" stroke="currentColor" />
    <path d="M7.5 11.5H13.5" stroke="currentColor" />
  </svg>
);

// 技能
export const SkillIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 17 17" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M4.57788 5.77124H10.7029" stroke="currentColor" />
    <path d="M4.57788 8.89819H7.91879" stroke="currentColor" />
    <path d="M12.1404 1.19446C12.9442 1.19446 13.6404 1.81999 13.6404 2.64465V8.89856H12.6404V2.64465C12.6404 2.42015 12.4411 2.19446 12.1404 2.19446H3.14038C2.83968 2.19446 2.64038 2.42015 2.64038 2.64465V13.0929C2.64082 13.3172 2.84001 13.5421 3.14038 13.5421H8.88159V14.5421H3.14038C2.33675 14.5421 1.6408 13.9172 1.64038 13.0929V2.64465C1.64038 1.81999 2.33651 1.19446 3.14038 1.19446H12.1404Z" fill="currentColor" />
    <path d="M12.0051 15.1056C12.0051 13.6395 10.8166 12.451 9.35059 12.451C10.8166 12.451 12.0051 11.2626 12.0051 9.79651C12.0051 11.2626 13.1936 12.451 14.6597 12.451C13.1936 12.451 12.0051 13.6395 12.0051 15.1056Z" stroke="currentColor" />
  </svg>
);

// 人设
export const UserIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M8 8.25C9.51878 8.25 10.75 7.01878 10.75 5.5C10.75 3.98122 9.51878 2.75 8 2.75C6.48122 2.75 5.25 3.98122 5.25 5.5C5.25 7.01878 6.48122 8.25 8 8.25Z" stroke="currentColor" />
    <path d="M2.5 14.5C2.5 11.5 5.25 10.25 8 10.25C10.75 10.25 13.5 11.5 13.5 14.5" stroke="currentColor" />
  </svg>
);

/*
 * 视图选项菜单图标。
 */

// 分组方式：按工作区
export const ViewGroupWorkspaceIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M1.50439 3.11059C1.50439 2.55831 1.95211 2.1106 2.50439 2.1106H5.43389C5.67773 2.1106 5.91318 2.19969 6.09593 2.36113L7.71649 3.79265C7.89924 3.95409 8.1347 4.04319 8.3785 4.04319H13.4958C14.0481 4.04319 14.4958 4.4909 14.4958 5.04319V12.8894C14.4958 13.4417 14.0481 13.8894 13.4958 13.8894H2.50439C1.95211 13.8894 1.50439 13.4417 1.50439 12.8894V4.04319V3.11059Z" stroke="currentColor" />
    <path d="M3.63501 7.66614H12.3647" stroke="currentColor" />
  </svg>
);

// 分组方式：按工作区树
export const ViewGroupTreeIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={className} {...props}>
    <path d="M14 12.05c0 .8-.65 1.45-1.46 1.45H3.46C2.65 13.5 2 12.85 2 12.05v-8.1c0-.8.65-1.45 1.46-1.45h2.4c.49 0 .94.24 1.21.65l.5.73c.27.4.73.65 1.21.65h3.76c.8 0 1.46.65 1.46 1.45v6.02Z" stroke="currentColor" />
    <path d="M8.7 8.1v3M11.2 8.1v3" stroke="currentColor" />
  </svg>
);

// 分组方式：单列表
export const ViewGroupFlatIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" aria-hidden="true" className={className} {...props}>
    <path d="M6 3.5h7.5M6 8h7.5M6 12.5h7.5" stroke="currentColor" />
    <path d="M2.6 3.5h.01M2.6 8h.01M2.6 12.5h.01" stroke="currentColor" />
  </svg>
);

// 排序方式：手动排序
export const ViewOrderManualIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={className} {...props}>
    <path d="m5.1 6 2.9-2.9L10.9 6" stroke="currentColor" />
    <path d="m5.1 10 2.9 2.9 2.9-2.9" stroke="currentColor" />
  </svg>
);

// 排序方式：最近更新
export const ViewOrderUpdatedIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={className} {...props}>
    <path d="M8 14C11.3137 14 14 11.3137 14 8C14 4.68629 11.3137 2 8 2C4.68629 2 2 4.68629 2 8C2 11.3137 4.68629 14 8 14Z" stroke="currentColor" />
    <path d="M8 4.31V8.46L11 10.08" stroke="currentColor" />
  </svg>
);

// 筛选会话：隐藏已归档
export const ViewFilterHiddenIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => {
  const maskId = `axon-archive-off-${React.useId().replaceAll(':', '')}`;
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={className} {...props}>
      <mask id={maskId} maskUnits="userSpaceOnUse" x="0" y="0" width="16" height="16">
        <rect x="0" y="0" width="16" height="16" fill="white" stroke="none" />
        <path d="m2.2 1.3 11.6 12.8" stroke="black" strokeWidth="4" />
      </mask>
      <g mask={`url(#${maskId})`}>
        <rect x="1.9" y="2.1" width="12.2" height="3.4" rx="1.1" stroke="currentColor" />
        <path d="M2.95 5.7v4.8a2.9 2.9 0 0 0 2.9 2.9h4.3a2.9 2.9 0 0 0 2.9-2.9V5.7" stroke="currentColor" />
      </g>
      <path d="m2.2 1.3 11.6 12.8" stroke="currentColor" />
    </svg>
  );
};

// 筛选会话：全部对话（含已归档）
export const ViewFilterAllIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={className} {...props}>
    <path d="M5 6.75H11" stroke="currentColor" />
    <path d="M5 9H8" stroke="currentColor" />
    <path d="M2.37067 11.2497C1.5872 9.89252 1.32042 8.29798 1.61945 6.7597C1.91847 5.22141 2.76317 3.84293 3.99801 2.87809C5.23285 1.91325 6.7747 1.427 8.33964 1.50888C9.90458 1.59076 11.3873 2.23526 12.5147 3.32369C13.6422 4.41232 14.3384 5.8717 14.4751 7.43304C14.6118 8.99438 14.1797 10.5525 13.2585 11.8205C12.3372 13.0885 10.9889 13.9809 9.4617 14.3334C8.18666 14.6277 6.8587 14.529 5.64964 14.0601C5.17095 13.8745 4.76937 13.4929 4.26509 13.3963C3.67389 13.2832 2.95232 13.5595 2.0377 14.3334" stroke="currentColor" />
  </svg>
);

// 筛选会话：仅显示已归档
export const ViewFilterOnlyIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={className} {...props}>
    <rect x="1.9" y="2.1" width="12.2" height="3.4" rx="1.1" stroke="currentColor" />
    <path d="M2.95 5.7v4.8a2.9 2.9 0 0 0 2.9 2.9h4.3a2.9 2.9 0 0 0 2.9-2.9V5.7" stroke="currentColor" />
    <path d="m6 9.35 1.4 1.4 2.6-2.6" stroke="currentColor" />
  </svg>
);

// 添加工作区
export const WorkspaceAddIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={className} {...props}>
    <path d="M5.54492 2.06738C5.91034 2.06754 6.26318 2.20149 6.53711 2.44336L7.94043 3.68164V4.7998C7.71462 4.74105 7.50367 4.63139 7.32617 4.47461L5.87598 3.19238C5.78477 3.11185 5.66658 3.06754 5.54492 3.06738H2.94922C2.67322 3.06738 2.44946 3.29145 2.44922 3.56738V12.4326C2.44927 12.7087 2.67311 12.9326 2.94922 12.9326H12.9326C13.2086 12.9325 13.4326 12.7086 13.4326 12.4326V8.53613H14.4326V12.4326C14.4326 13.2609 13.7609 13.9325 12.9326 13.9326H2.94922C2.12083 13.9326 1.44927 13.261 1.44922 12.4326V3.56738C1.44946 2.73916 2.12094 2.06738 2.94922 2.06738H5.54492Z" fill="currentColor" stroke="none" />
    <path d="M9.75977 4.50208H14.5509" stroke="currentColor" />
    <path d="M12.1492 6.89758L12.1492 2.10642" stroke="currentColor" />
  </svg>
);

export const SidebarIcon = IconPanelLeft;

// 展开箭头：实心三角，闭合朝右，展开旋转 90 度朝下
export const TriangleRightFillIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M5.5 4.5C5.5 4.40714 5.52586 4.31612 5.57467 4.23713C5.62349 4.15815 5.69334 4.09431 5.77639 4.05279C5.85945 4.01126 5.95242 3.99368 6.0449 4.00202C6.13738 4.01036 6.22572 4.04429 6.3 4.1L10.967 7.6C11.0291 7.64657 11.0795 7.70697 11.1142 7.77639C11.1489 7.84582 11.167 7.92238 11.167 8C11.167 8.07762 11.1489 8.15418 11.1142 8.22361C11.0795 8.29303 11.0291 8.35343 10.967 8.4L6.3 11.9C6.22572 11.9557 6.13738 11.9896 6.0449 11.998C5.95242 12.0063 5.85945 11.9887 5.77639 11.9472C5.69334 11.9057 5.62349 11.8419 5.57467 11.7629C5.52586 11.6839 5.5 11.5929 5.5 11.5V4.5Z" fill="currentColor" />
  </svg>
);

// 发送：向上箭头
// 发送：与既有工作台产品同款路径
export const ArrowUpIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M6.97211 1.94476C7.55785 1.35914 8.50767 1.35919 9.09343 1.94476L13.921 6.77228L13.2138 7.47939L8.38632 2.65187C8.19108 2.45682 7.87443 2.45677 7.67922 2.65187L2.74397 7.58711L2.03687 6.88L6.97211 1.94476Z" fill="currentColor" />
    <path d="M7.97571 14.5732L8.02421 2.34139" stroke="currentColor" />
  </svg>
);

// 停止生成：圆角方块
// 停止：与既有工作台产品同款路径
export const StopIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M12.5 2.5H3.5C2.94772 2.5 2.5 2.94772 2.5 3.5V12.5C2.5 13.0523 2.94772 13.5 3.5 13.5H12.5C13.0523 13.5 13.5 13.0523 13.5 12.5V3.5C13.5 2.94772 13.0523 2.5 12.5 2.5Z" fill="currentColor" />
  </svg>
);

// 右侧面板开关：采用既有工作台产品的左侧面板图形镜像，分隔线落在右缘。
// 镜像写在图形内部，避免与外层传入的 style 或 transform 打架。
export const PanelRightIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <g transform="translate(16 0) scale(-1 1)">
      <path d="M13.5 1.5H2.5C1.94772 1.5 1.5 1.94772 1.5 2.5V13.5C1.5 14.0523 1.94772 14.5 2.5 14.5H13.5C14.0523 14.5 14.5 14.0523 14.5 13.5V2.5C14.5 1.94772 14.0523 1.5 13.5 1.5Z" stroke="currentColor" />
      <path d="M5.5 1.5V14.5" stroke="currentColor" />
    </g>
  </svg>
);

// 分栏：一列拆成两列，与既有工作台产品的分栏控件同款路径
export const SplitIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M13.5 1.5H2.5C1.94772 1.5 1.5 1.94772 1.5 2.5V13.5C1.5 14.0523 1.94772 14.5 2.5 14.5H13.5C14.0523 14.5 14.5 14.0523 14.5 13.5V2.5C14.5 1.94772 14.0523 1.5 13.5 1.5Z" stroke="currentColor" />
    <path d="M8 1.5V14.5" stroke="currentColor" />
  </svg>
);

// 全屏：四角外扩，与既有工作台产品的面板全屏控件同款路径
export const FullscreenIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden="true" className={className} {...props}>
    <path d="M2.33203 10.4054V13.1681C2.33229 13.444 2.55605 13.6681 2.83203 13.6681H5.49512V14.6681H2.83203C2.00376 14.6681 1.33229 13.9963 1.33203 13.1681V10.4054H2.33203ZM14.6689 13.1681C14.6687 13.996 13.9968 14.6676 13.1689 14.6681H10.4951V13.6681H13.1689C13.4445 13.6676 13.6687 13.4437 13.6689 13.1681V10.4054H14.6689V13.1681ZM13.1689 1.33118C13.9969 1.33163 14.6688 2.00315 14.6689 2.83118V5.4054H13.6689V2.83118C13.6688 2.55544 13.4446 2.33162 13.1689 2.33118H10.4951V1.33118H13.1689ZM5.49512 2.33118H2.83203C2.55598 2.33118 2.33218 2.55516 2.33203 2.83118V5.4054H1.33203V2.83118C1.33218 2.00288 2.00369 1.33118 2.83203 1.33118H5.49512V2.33118Z" fill="currentColor" />
  </svg>
);

// 退出全屏：四角内收，与全屏图标成对
export const ExitFullscreenIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden="true" className={className} {...props}>
    <path d="M9 2.5V6C9 6.26522 9.10536 6.51957 9.29289 6.70711C9.48043 6.89464 9.73478 7 10 7H13.5" stroke="currentColor" />
    <path d="M7 13.5V10C7 9.73478 6.89464 9.48043 6.70711 9.29289C6.51957 9.10536 6.26522 9 6 9H2.5" stroke="currentColor" />
  </svg>
);

// 后台作业：终端
export const TerminalIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={className} {...props}>
    <path d="M2.5 3.5H13.5V12.5H2.5V3.5Z" stroke="currentColor" />
    <path d="M4.75 6.5L6.5 8L4.75 9.5" stroke="currentColor" />
    <path d="M8 9.75H11" stroke="currentColor" />
  </svg>
);

// 文件
// 文件：与既有工作台产品同款路径
export const FileIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M6.15479 4.91687H9.84543" stroke="currentColor" />
    <path d="M11.8798 9.55347V2.71525C11.8798 2.37416 11.564 2.09766 11.1744 2.09766H4.82577C4.43618 2.09766 4.12036 2.37416 4.12036 2.71525V9.55347" stroke="currentColor" />
    <path d="M2.28735 13.8022V8.84792C2.28735 8.77514 2.36262 8.72673 2.42884 8.75693L13.2936 13.7112C13.3914 13.7558 13.3596 13.9022 13.2521 13.9022H2.38735C2.33213 13.9022 2.28735 13.8575 2.28735 13.8022Z" stroke="currentColor" />
    <path d="M7.46929 10.979L13.5783 8.7416C13.6435 8.7177 13.7126 8.76601 13.7126 8.83551L13.7125 13.8022C13.7125 13.8574 13.6678 13.9022 13.6125 13.9022H7.99999" stroke="currentColor" />
    <path d="M6.15479 7.2395H9.05644" stroke="currentColor" />
  </svg>
);

// 通用工具调用
// 工具调用：采用既有工作台产品的检视图形（角括号加斜杠），与它的「查看」入口同款
export const ToolIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M4.67398 4.25061L1.36094 7.86484C1.29085 7.9413 1.29085 8.05866 1.36094 8.13513L4.67398 11.7494" stroke="currentColor" />
    <path d="M11.3262 4.25061L14.6392 7.86484C14.7093 7.9413 14.7093 8.05866 14.6392 8.13513L11.3262 11.7494" stroke="currentColor" />
    <path d="M9.56222 3.62573L6.43774 12.3743" stroke="currentColor" />
  </svg>
);

// 子代理：分叉
// 子代理：与既有工作台产品同款路径
export const AgentIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M1.01503 8.0001L5.6964 8.0001C6.41913 8.0001 6.78049 8.0001 7.12115 7.91951C7.4232 7.84804 7.71233 7.73014 7.97821 7.57C8.27809 7.38939 8.5364 7.13669 9.05303 6.63129L11.3281 4.40564" stroke="currentColor" />
    <path d="M1.01221 7.9999L5.6964 7.9999C6.41913 7.9999 6.78049 7.9999 7.12115 8.08049C7.4232 8.15196 7.71233 8.26986 7.97821 8.43C8.27809 8.61061 8.5364 8.86331 9.05303 9.36871L11.3281 11.5944" stroke="currentColor" />
    <circle cx="12.4502" cy="3.3079" r="1.56962" stroke="currentColor" />
    <circle cx="12.4502" cy="12.6921" r="1.56962" stroke="currentColor" />
  </svg>
);

// 复制：与既有工作台产品同款，圆角外框加叠层轮廓
export const CopyIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <rect x="1.52075" y="4.07373" width="10.3932" height="10.3932" rx="2" stroke="currentColor" />
    <path d="M11.9792 1.53296C13.36 1.53296 14.4792 2.65225 14.4792 4.03296V9.42847C14.4792 10.3756 13.9521 11.1987 13.1755 11.6228V10.3298C13.3652 10.0787 13.4792 9.7674 13.4792 9.42847V4.03296C13.4792 3.20453 12.8077 2.53296 11.9792 2.53296H6.58374C6.27966 2.53301 5.99684 2.6235 5.7605 2.77905H4.42358C4.85652 2.03463 5.66056 1.53304 6.58374 1.53296H11.9792Z" fill="currentColor" />
  </svg>
);

// 下载
export const DownloadIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={className} {...props}>
    <path d="M14 10V12.5C14 13.3284 13.3284 14 12.5 14H3.5C2.67157 14 2 13.3284 2 12.5V10" stroke="currentColor" />
    <path d="M8 2V11M8 11L11.5 7.5M8 11L4.5 7.5" stroke="currentColor" />
  </svg>
);

// 展开箭头：细线朝右
// 右箭头：与既有工作台产品同款路径
export const ChevronRightIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M6 12L9.29289 8.70711C9.68342 8.31658 9.68342 7.68342 9.29289 7.29289L6 4" stroke="currentColor" />
  </svg>
);

// 收起箭头：细线朝左，与 ChevronRightIcon 是同一笔的镜像
// 左箭头：右箭头图形的镜像，镜像写在图形内部，避免与外层传入的 style 或 transform 打架
export const ChevronLeftIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <g transform="translate(16 0) scale(-1 1)">
      <path d="M6 12L9.29289 8.70711C9.68342 8.31658 9.68342 7.68342 9.29289 7.29289L6 4" stroke="currentColor" />
    </g>
  </svg>
);

// 重试
export const RetryIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={className} {...props}>
    <path d="M13.25 7.25C13.0916 6.16276 12.5965 5.15288 11.8336 4.35817C11.0708 3.56345 10.0791 3.0241 8.99704 2.81511C7.91499 2.60612 6.7964 2.73781 5.79099 3.19219C4.78559 3.64656 3.94515 4.40081 3.38653 5.3502C2.8279 6.29958 2.57911 7.39563 2.67432 8.49051C2.76953 9.58538 3.20388 10.6222 3.91648 11.4606C4.62908 12.2991 5.58357 12.8957 6.64801 13.1688C7.71245 13.442 8.83253 13.3787 9.85913 12.9878" stroke="currentColor" />
    <path d="M13.5 3.5V7.5H9.5" stroke="currentColor" />
  </svg>
);

// 思考：与既有工作台产品同款路径（两个交错的环形加中心点，描边不填充）
export const BrainIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M10.2854 5.71481C12.9673 8.39663 14.1182 11.5938 12.8562 12.8559C11.5942 14.1179 8.39706 12.9669 5.71518 10.2851C3.03333 7.60323 1.88236 4.40608 3.14441 3.14403C4.40644 1.882 7.6036 3.03297 10.2854 5.71481Z" stroke="currentColor" />
    <path d="M10.2854 10.2851C7.6036 12.9669 4.40644 14.1179 3.14441 12.8559C1.88236 11.5938 3.03333 8.39663 5.71518 5.71481C8.39706 3.03297 11.5942 1.882 12.8562 3.14403C14.1182 4.40608 12.9673 7.60323 10.2854 10.2851Z" stroke="currentColor" />
    <path d="M8.86291 8.0002C8.86291 8.47549 8.47762 8.86087 8.00224 8.86087C7.52694 8.86087 7.1416 8.47549 7.1416 8.0002C7.1416 7.52485 7.52694 7.13953 8.00224 7.13953C8.47762 7.13953 8.86291 7.52485 8.86291 8.0002Z" fill="currentColor" />
  </svg>
);

// 仪表盘：用于耗时、速率一类连续量（图形数据迁移自既有工作台产品，下同）
export const GaugeIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M3.4041 13.096C2.49514 12.187 1.87614 11.0288 1.62537 9.76798C1.37459 8.50716 1.50331 7.20028 1.99525 6.01261C2.48719 4.82494 3.32025 3.80981 4.3891 3.09557C5.45795 2.38134 6.71458 2.00008 8.0001 2C9.28563 2.00008 10.5423 2.38134 11.6111 3.09557C12.68 3.80981 13.513 4.82494 14.005 6.01261C14.4969 7.20028 14.6256 8.50716 14.3748 9.76798C14.1241 11.0288 13.5051 12.187 12.5961 13.096" stroke="currentColor" />
    <path d="M8 8.49994L11.6114 4.88855" stroke="currentColor" />
    <path d="M8 9.75C8.69036 9.75 9.25 9.19036 9.25 8.5C9.25 7.80964 8.69036 7.25 8 7.25C7.30964 7.25 6.75 7.80964 6.75 8.5C6.75 9.19036 7.30964 9.75 8 9.75Z" fill="currentColor" />
  </svg>
);

// 数据层：用于 Token 与缓存一类累计量
export const DatabaseIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M13.1967 5.1869C13.7232 4.77378 14.0003 4.30517 14.0001 3.82819C14.0003 3.3512 13.7232 2.88259 13.1967 2.46947C12.6702 2.05635 11.9128 1.71328 11.0006 1.47475C10.0885 1.23621 9.05371 1.11062 8.00039 1.1106C6.94707 1.11057 5.9123 1.23612 5.00009 1.47461C4.08742 1.71301 3.32948 2.05604 2.80249 2.46919C2.2755 2.88235 1.99805 3.35106 1.99805 3.82819C1.99805 4.30531 2.2755 4.77402 2.80249 5.18718C3.32948 5.60033 4.08742 5.94336 5.00009 6.18176C5.9123 6.42025 6.94707 6.5458 8.00039 6.54578C9.05371 6.54575 10.0885 6.42016 11.0006 6.18163C11.9128 5.94309 12.6702 5.60002 13.1967 5.1869Z" stroke="currentColor" />
    <path d="M2 3.80371V11.7848" stroke="currentColor" />
    <path d="M14 3.80371V11.7848" stroke="currentColor" />
    <path d="M2 7.81396C2 8.60524 2.63214 9.36411 3.75736 9.92363C4.88258 10.4832 6.4087 10.7975 8 10.7975C9.5913 10.7975 11.1174 10.4832 12.2426 9.92363C13.3679 9.36411 14 8.60524 14 7.81396" stroke="currentColor" />
    <path d="M2 11.7847C2 12.6081 2.63214 13.3977 3.75736 13.98C4.88258 14.5622 6.4087 14.8893 8 14.8893C9.5913 14.8893 11.1174 14.5622 12.2426 13.98C13.3679 13.3977 14 12.6081 14 11.7847" stroke="currentColor" />
  </svg>
);

// 时钟：用于首字与耗时
export const ClockIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M8 14C11.3137 14 14 11.3137 14 8C14 4.68629 11.3137 2 8 2C4.68629 2 2 4.68629 2 8C2 11.3137 4.68629 14 8 14Z" stroke="currentColor" />
    <path d="M8 4.31V8.46L11 10.08" stroke="currentColor" />
  </svg>
);

// 圆环进度：用于上下文占用
export const RingIcon: React.FC<IconProps & { ratio?: number }> = ({
  size = 14,
  className = '',
  ratio = 0,
  ...props
}) => {
  const clamped = Math.max(0, Math.min(1, ratio));
  const radius = 5.5;
  const circumference = 2 * Math.PI * radius;
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden="true" className={className} {...props}>
      <circle cx="8" cy="8" r={radius} stroke="currentColor" strokeOpacity="0.25" strokeWidth="1.5" />
      <circle
        cx="8"
        cy="8"
        r={radius}
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeDasharray={`${circumference * clamped} ${circumference}`}
        transform="rotate(-90 8 8)"
      />
    </svg>
  );
};

// 清单：用于计划进度
// 清单：与既有工作台产品同款路径
export const ListIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M3.75 6.25C4.7165 6.25 5.5 5.4665 5.5 4.5C5.5 3.5335 4.7165 2.75 3.75 2.75C2.7835 2.75 2 3.5335 2 4.5C2 5.4665 2.7835 6.25 3.75 6.25Z" stroke="currentColor" />
    <path d="M7.5 4.5H13.5" stroke="currentColor" />
    <path d="M3.75 13.25C4.7165 13.25 5.5 12.4665 5.5 11.5C5.5 10.5335 4.7165 9.75 3.75 9.75C2.7835 9.75 2 10.5335 2 11.5C2 12.4665 2.7835 13.25 3.75 13.25Z" stroke="currentColor" />
    <path d="M7.5 11.5H13.5" stroke="currentColor" />
  </svg>
);

// 代码：用于 LSP 与代码相关指标
// 代码：与既有工作台产品同款路径
export const CodeIcon: React.FC<IconProps> = ({ size = 16, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M6.27612 1.5L4.52612 14.5" stroke="currentColor" />
    <path d="M11.4739 1.5L9.72388 14.5" stroke="currentColor" />
    <path d="M2.39868 5.5H14.0681" stroke="currentColor" />
    <path d="M1.93188 10.5H13.6013" stroke="currentColor" />
  </svg>
);

// 刷新：重新读取当前面板数据，与既有工作台产品同款路径
export const RefreshIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M14.5001 8C14.5 9.28552 14.1188 10.5422 13.4045 11.611C12.6903 12.6799 11.6752 13.5129 10.4875 14.0049C9.29982 14.4968 7.99295 14.6255 6.73212 14.3747C5.4713 14.124 4.31314 13.505 3.4041 12.596C2.49514 11.687 1.87614 10.5288 1.62537 9.26798C1.37459 8.00716 1.50331 6.70028 1.99525 5.51261C2.48719 4.32494 3.32025 3.30981 4.3891 2.59557C5.45795 1.88134 6.71458 1.50008 8.0001 1.5C9.9001 1.5 11.7001 2.3 13.0001 3.6L14.5001 5.1" stroke="currentColor" />
    <path d="M14.4999 1.5V5.1H10.8999" stroke="currentColor" />
  </svg>
);

// 自动换行：文本在行尾折返
export const WrapIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden="true" className={className} {...props}>
    <path d="M10.9999 8C10.9999 6.89543 10.1046 6 9 6H4.5V5H9C10.6568 5 11.9999 6.34315 11.9999 8C11.9999 9.65685 10.6568 11 9 11H6.20703L6.85351 11.6465L6.14648 12.3535L4.64652 10.8536C4.45126 10.6583 4.45126 10.3417 4.64652 10.1464L6.14648 8.64648L6.85351 9.35352L6.20703 10H9C10.1046 10 10.9999 9.10457 10.9999 8Z" fill="currentColor" />
    <path d="M2 15H1V1H2V15Z" fill="currentColor" />
    <path d="M15 15H14V1H15V15Z" fill="currentColor" />
  </svg>
);

// 取消换行：超长行改为横向滚动
export const NoWrapIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden="true" className={className} {...props}>
    <path d="M2 15H1V1H2V15Z" fill="currentColor" />
    <path d="M12.3535 7.64645C12.5487 7.84171 12.5487 8.15829 12.3535 8.35355L9.85352 10.8535L9.14648 10.1465L10.793 8.5H3.5V7.5H10.793L9.14648 5.85352L9.85352 5.14648L12.3535 7.64645Z" fill="currentColor" />
    <path d="M15 15H14V1H15V15Z" fill="currentColor" />
  </svg>
);

// 用其他应用打开：右上角的引出箭头，与既有工作台产品的同款路径
export const OpenInAppIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true" className={className} {...props}>
    <path d="M11.7256 2.77441C12.5538 2.77469 13.2256 3.44616 13.2256 4.27441V10.1416H12.2256V4.27441C12.2256 3.99844 12.0015 3.77469 11.7256 3.77441H5.7207V2.77441H11.7256Z" fill="currentColor" />
    <path d="M2.77441 13.2255L12.3756 3.62427" stroke="currentColor" />
  </svg>
);

// 运行时：活动脉冲
export const PulseIcon: React.FC<IconProps> = ({ size = 14, className = '', ...props }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className={className} {...props}>
    <path d="M2 8H4.5L6 4.5L8.5 11.5L10 8H14" stroke="currentColor" />
  </svg>
);
