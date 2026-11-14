// CSP 兼容垫片：react-remove-scroll 运行期注入 <style> 元素锁定滚动，
// 违反本服务的 style-src 'self'。工作台 body 本身 overflow:hidden，无需滚动锁，
// 这里只透传子元素（Radix Dialog/Popover/Menu 的 ref 由子元素自身组合）。
import type { ReactNode } from 'react';

type Props = { children?: ReactNode; [key: string]: unknown };

export function RemoveScroll({ children }: Props) {
  return <>{children}</>;
}
RemoveScroll.classNames = { fullWidth: 'width-before-scroll-bar', zeroRight: 'right-scroll-bar-position' };

export default RemoveScroll;
