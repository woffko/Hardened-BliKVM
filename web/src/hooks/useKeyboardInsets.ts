import { useEffect, useState, type RefObject } from 'react';

// How much of the page the system keyboard covers at the bottom while
// `active`. Browsers that honour interactive-widget=resizes-content
// (index.html), like Chrome on Android, shrink the layout above the keyboard
// and this stays 0; others (Safari on iOS, older Firefox) keep the layout and
// shrink only the visual viewport, and the difference is the keyboard.
export function useSystemKeyboardInset(active: boolean) {
  const [inset, setInset] = useState(0);

  useEffect(() => {
    const viewport = window.visualViewport;
    if (!active || !viewport) {
      setInset(0);
      return;
    }
    const visual = viewport;

    function update() {
      // A pinch zoom of the page shrinks the visual viewport too; it is no
      // keyboard.
      if (Math.abs(visual.scale - 1) > 0.01) {
        setInset(0);
        return;
      }
      setInset(Math.max(0, Math.round(window.innerHeight - visual.height - visual.offsetTop)));
    }

    update();
    visual.addEventListener('resize', update);
    visual.addEventListener('scroll', update);
    return () => {
      visual.removeEventListener('resize', update);
      visual.removeEventListener('scroll', update);
      setInset(0);
    };
  }, [active]);

  return inset;
}

// Reports the height of the element in `ref`, plus `extra`, while `active`
// (the element is rendered), and 0 otherwise or once it is gone.
export function useReportHeight(
  ref: RefObject<HTMLElement | null>,
  active: boolean,
  report: (height: number) => void,
  extra = 0
) {
  useEffect(() => {
    const element = ref.current;
    if (!active || !element) {
      report(0);
      return;
    }

    const update = () => report(Math.round(element.getBoundingClientRect().height) + extra);
    update();
    if (typeof ResizeObserver === 'undefined') {
      return () => report(0);
    }
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => {
      observer.disconnect();
      report(0);
    };
  }, [ref, active, report, extra]);
}
