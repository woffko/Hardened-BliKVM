import { useEffect, useState } from 'react';
import { useAtom, useAtomValue, useSetAtom } from 'jotai';
import { CheckIcon, MonitorIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { setScreenArea } from '@/api/hid.ts';
import { isScreenArea, type ScreenArea } from '@/lib/screen-area.ts';
import { isKeyboardEnableAtom } from '@/jotai/keyboard.ts';
import { screenAreaAtom } from '@/jotai/mouse.ts';
import { resolutionAtom } from '@/jotai/screen.ts';
import { agentScreenAreaAtom } from '@/jotai/target-agent.ts';
import { MenuSubmenu } from '@/components/menu-submenu.tsx';

type Typed = Record<'desktopWidth' | 'desktopHeight' | 'x' | 'y' | 'width' | 'height', string>;

const typedOf = (area: ScreenArea): Typed => ({
  desktopWidth: String(area.desktop.width),
  desktopHeight: String(area.desktop.height),
  x: String(area.screen.x),
  y: String(area.screen.y),
  width: String(area.screen.width),
  height: String(area.screen.height)
});

const areaOf = (typed: Typed): ScreenArea | null => {
  const number = (text: string) => (/^\d{1,5}$/.test(text.trim()) ? Number(text) : NaN);
  const area = {
    desktop: { width: number(typed.desktopWidth), height: number(typed.desktopHeight) },
    screen: {
      x: number(typed.x),
      y: number(typed.y),
      width: number(typed.width),
      height: number(typed.height)
    }
  };
  return isScreenArea(area) ? area : null;
};

// Where the captured screen lies in the target's desktop
// (docs/design/screen-area-v1.md): a target that extends its desktop over
// more than one monitor spreads the absolute mouse over all of them. The
// whole desktop, or the desktop's size and this screen's place in it, in
// the target's pixels; the board keeps it for every browser. While the
// target agent's monitors make a layout, it is theirs, and nothing is
// offered: duplicated monitors are one screen, not part of a desktop.
export const DesktopArea = () => {
  const { t } = useTranslation();
  const [area, setArea] = useAtom(screenAreaAtom);
  const agent = useAtomValue(agentScreenAreaAtom);
  const resolution = useAtomValue(resolutionAtom);
  const [typed, setTyped] = useState<Typed>(() =>
    typedOf(
      area ?? {
        desktop: { width: 2 * (resolution?.width || 1920), height: resolution?.height || 1080 },
        screen: { x: 0, y: 0, width: resolution?.width || 1920, height: resolution?.height || 1080 }
      }
    )
  );
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const typedArea = areaOf(typed);

  // While a field has the focus its keys are the page's, not the target's.
  const setKeyboardEnabled = useSetAtom(isKeyboardEnableAtom);
  const [typing, setTyping] = useState(false);
  useEffect(() => {
    if (!typing) return;
    setKeyboardEnabled(false);
    return () => setKeyboardEnabled(true);
  }, [typing, setKeyboardEnabled]);

  useEffect(() => {
    if (area) setTyped(typedOf(area));
  }, [area]);

  function save(next: ScreenArea | null) {
    if (saving) return;
    setSaving(true);
    setError('');
    setScreenArea(next)
      .then((rsp) => {
        if (rsp.code !== 0) throw new Error(rsp.msg || t('mouse.desktop.failed'));
        setArea(isScreenArea(rsp.data?.area) ? rsp.data.area : null);
      })
      .catch((err) => setError(err instanceof Error ? err.message : t('mouse.desktop.failed')))
      .finally(() => setSaving(false));
  }

  /** Two monitors of this screen's size side by side, this one on `side`. */
  function besideAnother(side: 'left' | 'right') {
    const width = Number(typed.width);
    const height = Number(typed.height);
    if (!(width > 0 && height > 0)) return;
    setTyped({
      desktopWidth: String(2 * width),
      desktopHeight: String(height),
      x: side === 'left' ? '0' : String(width),
      y: '0',
      width: String(width),
      height: String(height)
    });
  }

  const field = (key: keyof Typed, label: string) => (
    <input
      key={key}
      aria-label={label}
      data-desktop-area={key}
      className="w-[56px] rounded bg-neutral-800 px-1 text-sm text-neutral-200"
      inputMode="numeric"
      value={typed[key]}
      onFocus={() => setTyping(true)}
      onBlur={() => setTyping(false)}
      onChange={(event) => setTyped({ ...typed, [key]: event.target.value })}
    />
  );

  const entry = (key: string, chosen: boolean, label: string, onClick: () => void) => (
    <div
      key={key}
      data-desktop-area-entry={key}
      className="flex h-[30px] cursor-pointer select-none items-center rounded pl-1 pr-5 hover:bg-neutral-700/70"
      onClick={onClick}
    >
      <div className="flex h-[14px] w-[20px] items-end text-blue-500">
        {chosen && <CheckIcon size={14} />}
      </div>
      <span>{label}</span>
    </div>
  );

  const fromAgent = agent !== null && (
    <div className="flex flex-col">
      {entry(
        'agent',
        true,
        agent === 'whole'
          ? t('mouse.desktop.fromAgentWhole')
          : t('mouse.desktop.fromAgentPart', {
              width: agent.desktop.width,
              height: agent.desktop.height,
              x: agent.screen.x,
              y: agent.screen.y
            }),
        () => undefined
      )}
      <div className="max-w-[280px] px-2 pb-1 pt-1 text-xs text-neutral-400">
        {t('mouse.desktop.fromAgentTip')}
      </div>
    </div>
  );

  const content = fromAgent || (
    <div className="flex flex-col">
      {entry('whole', area === null, t('mouse.desktop.whole'), () => save(null))}
      {entry(
        'part',
        area !== null,
        area
          ? t('mouse.desktop.partOf', {
              width: area.desktop.width,
              height: area.desktop.height,
              x: area.screen.x,
              y: area.screen.y
            })
          : t('mouse.desktop.part'),
        () => typedArea && save(typedArea)
      )}
      <form
        className="flex flex-col gap-1 px-2 py-1"
        onSubmit={(event) => {
          event.preventDefault();
          if (typedArea) save(typedArea);
        }}
      >
        <div className="flex items-center gap-1">
          <span className="w-[86px] text-xs text-neutral-400">{t('mouse.desktop.desktop')}</span>
          {field('desktopWidth', t('mouse.desktop.desktopWidth'))}
          <span className="text-neutral-400">×</span>
          {field('desktopHeight', t('mouse.desktop.desktopHeight'))}
        </div>
        <div className="flex items-center gap-1">
          <span className="w-[86px] text-xs text-neutral-400">{t('mouse.desktop.screen')}</span>
          {field('width', t('mouse.desktop.screenWidth'))}
          <span className="text-neutral-400">×</span>
          {field('height', t('mouse.desktop.screenHeight'))}
        </div>
        <div className="flex items-center gap-1">
          <span className="w-[86px] text-xs text-neutral-400">{t('mouse.desktop.at')}</span>
          {field('x', t('mouse.desktop.screenX'))}
          <span className="text-neutral-400">,</span>
          {field('y', t('mouse.desktop.screenY'))}
        </div>
        <div className="flex items-center gap-1">
          <button
            type="button"
            data-desktop-area-beside="left"
            className="rounded px-2 text-xs text-blue-400 hover:bg-neutral-700/70"
            onClick={() => besideAnother('left')}
          >
            {t('mouse.desktop.leftOfTwo')}
          </button>
          <button
            type="button"
            data-desktop-area-beside="right"
            className="rounded px-2 text-xs text-blue-400 hover:bg-neutral-700/70"
            onClick={() => besideAnother('right')}
          >
            {t('mouse.desktop.rightOfTwo')}
          </button>
          <button
            type="submit"
            disabled={!typedArea || saving}
            className="ml-auto rounded px-2 text-sm text-blue-400 hover:bg-neutral-700/70 disabled:text-neutral-500"
          >
            {t('mouse.desktop.apply')}
          </button>
        </div>
      </form>
      <div className="max-w-[280px] px-2 pb-1 pt-1 text-xs text-neutral-400">
        {t('mouse.desktop.tip')}
      </div>
      {error && (
        <div role="alert" className="max-w-[280px] px-2 pb-1 text-xs text-red-400">
          {error}
        </div>
      )}
    </div>
  );

  return (
    <MenuSubmenu
      icon={<MonitorIcon size={18} />}
      label={t('mouse.desktop.title')}
      content={content}
    />
  );
};
