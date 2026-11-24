import React from 'react';

/*
 * 引导页自己的美术件：罗盘、立方体与两个入口图形。
 *
 * 这四件都不是通用图标：罗盘只出现在引导页（页签与顶部水印各一次），
 * 立方体是「这个入口没有自带图形」的占位，两个入口图形以前带固定品牌色，
 * 现在与全站图标同源：一律吃 currentColor，深浅由所在位置决定。
 * 因此它们留在引导页自己的模块里，不进通用图标库。
 */

/** 图标尺寸与附加类名。 */
interface ArtworkProps {
  size?: number;
  className?: string;
}

/**
 * 罗盘：一枚圆环加指向东北的菱形指针。
 *
 * 画在 `currentColor` 上，因此页签取弱化墨色、顶部水印取更浅的水印色，
 * 两处共用同一份路径。
 */
export const CompassIcon: React.FC<ArtworkProps> = ({ size = 16, className = '' }) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 16 16"
    fill="none"
    aria-hidden="true"
    className={className}
  >
    <path
      d="M8 14C11.3137 14 14 11.3137 14 8C14 4.68629 11.3137 2 8 2C4.68629 2 2 4.68629 2 8C2 11.3137 4.68629 14 8 14Z"
      stroke="currentColor"
    />
    <path d="M10.6101 5.39014L8.99014 8.99014L5.39014 10.6101L7.01014 7.01014L10.6101 5.39014Z" fill="currentColor" />
  </svg>
);

/** 立方体：等轴测的方箱轮廓，作为没有自带图形的入口的占位。 */
export const CubeIcon: React.FC<ArtworkProps> = ({ size = 16, className = '' }) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 16 16"
    fill="none"
    aria-hidden="true"
    className={className}
  >
    <path
      d="M7.99998 2.5L12.9 5.2V10.8L7.99998 13.5L3.09998 10.8V5.2L7.99998 2.5Z"
      stroke="currentColor"
      strokeLinejoin="round"
    />
    <path
      d="M3.09998 5.19995L7.99998 7.89995M7.99998 7.89995L12.9 5.19995M7.99998 7.89995V13.5"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
    />
  </svg>
);

/**
 * 工作区文件入口的图形：一只文件夹。
 *
 * 主形吃 currentColor，后片用同色的低透明度压出层次，因此不再带固定琥珀色；
 * 换到任何底色上都与旁边的文字同源。
 */
export const GuideArtworkFiles: React.FC<ArtworkProps> = ({ size = 24, className = '' }) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 36 36"
    fill="none"
    aria-hidden="true"
    className={className}
  >
    <path
      d="M10.7603 27.922H24.6817C26.3441 27.922 27.1753 27.922 27.8102 27.5984C28.3687 27.3139 28.8228 26.8598 29.1074 26.3012C29.4309 25.6663 29.4309 24.8351 29.4309 23.1727V15.4936"
      stroke="currentColor"
      strokeOpacity="0.55"
      strokeWidth="1.97886"
    />
    <path
      d="M13.1597 8.07812C13.4182 8.07818 13.6727 8.14336 13.8989 8.26855L16.7554 9.84961C16.9817 9.97485 17.2369 10.041 17.4956 10.041H26.106C26.9492 10.0412 27.6323 10.7251 27.6323 11.5684V24.5371C27.6323 25.3805 26.9483 26.0645 26.105 26.0645H8.09619C7.25281 26.0645 6.56884 25.3805 6.56885 24.5371V9.60449C6.56909 8.76133 7.25297 8.07812 8.09619 8.07812H13.1597ZM9.81592 14.5508V16.5293H24.3999V14.5508H9.81592Z"
      fill="currentColor"
    />
  </svg>
);

/** 新建终端入口的图形：一枚提示符，同样吃 currentColor。 */
export const GuideArtworkTerminal: React.FC<ArtworkProps> = ({ size = 24, className = '' }) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 36 36"
    fill="none"
    aria-hidden="true"
    className={className}
  >
    <path d="M10 11L16.606 17.606C16.6841 17.6841 16.6841 17.8107 16.606 17.8888L10 24.4948" stroke="currentColor" strokeWidth="3.5" />
    <path d="M20.1211 24.4946H26.8685" stroke="currentColor" strokeWidth="3.5" />
  </svg>
);
