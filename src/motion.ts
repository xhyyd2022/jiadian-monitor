/**
 * 轻量切换动效：只做「进入」不做「等待」。
 *
 * 之前考虑过 Vue <Transition mode="out-in">，但 out-in 会先把旧页面移出、
 * 等动画结束再挂新页面，快速连点时会卡住输入。这里直接用 Web Animations API
 * 在已经渲染好的新内容上跑一段合成器动画，点击立即生效，动画只负责视觉过渡。
 */

const DURATION = 160;
const EASING = "cubic-bezier(0.2, 0.7, 0.2, 1)";

function reducedMotion(): boolean {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

/** 新内容淡入并轻微上移；重复调用会从当前位置重新开始，不会叠加。 */
export function paneIn(el: Element | null | undefined, distance = 5): void {
  if (!el || reducedMotion()) return;
  const node = el as HTMLElement;
  // 只取消同一元素上由本工具启动的动画，避免影响 Element Plus 自己的过渡。
  for (const anim of node.getAnimations()) {
    if (anim.effect?.getTiming().duration === DURATION) anim.cancel();
  }
  node.animate(
    [
      { opacity: 0, transform: `translateY(${distance}px)` },
      { opacity: 1, transform: "translateY(0)" },
    ],
    { duration: DURATION, easing: EASING },
  );
}

/** 切换指示条：只改 transform/width，交给合成器，不触发布局重排。 */
export const INK_DURATION = 180;
export const INK_EASING = EASING;
