import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
  type SyntheticEvent,
  type WheelEvent
} from 'react';
import { WindowsOutlined } from '@ant-design/icons';
import clsx from 'clsx';
import { useSetAtom } from 'jotai';
import {
  ArrowBigUpIcon,
  ArrowDownIcon,
  ArrowLeftIcon,
  ArrowRightIcon,
  ArrowRightToLineIcon,
  ArrowUpIcon,
  ChevronFirstIcon,
  ChevronLastIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  ChevronsDownIcon,
  ChevronsUpIcon,
  ChevronUpIcon,
  CommandIcon,
  CornerDownLeftIcon,
  EllipsisIcon,
  KeyboardOffIcon,
  OptionIcon
} from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { ModifierModes } from '@/lib/sticky-modifiers.ts';
import { keyHelperHeightAtom } from '@/jotai/keyboard.ts';
import { useReportHeight, useSystemKeyboardInset } from '@/hooks/useKeyboardInsets.ts';

// The keys a phone keyboard lacks, as one row that scrolls sideways, docked
// above the system keyboard while the mobile keyboard is open. The layout
// follows the key row of the RustAdmin mobile client: the modifiers first,
// then groups of keys, with Fn and "more" opening further keys in place.

type HelperKeyDef = {
  // The key code, or for a shortcut its name.
  id: string;
  label: ReactNode;
  // Spoken name, for a label that is an icon or an abbreviation.
  name?: string;
  // A shortcut: these modifiers go with the key `code`.
  combo?: { modifiers: string[]; code: string };
};

const ICON = 18;
// Width of the fade at an edge of the row that has more keys beyond it.
const EDGE_FADE_PX = 20;

function modifierKeys(isMac: boolean): HelperKeyDef[] {
  return [
    { id: 'ShiftLeft', label: <ArrowBigUpIcon size={ICON} />, name: 'Shift' },
    {
      id: 'ControlLeft',
      label: isMac ? <ChevronUpIcon size={ICON} /> : 'Ctrl',
      name: 'Control'
    },
    {
      id: 'AltLeft',
      label: isMac ? <OptionIcon size={ICON} /> : 'Alt',
      name: isMac ? 'Option' : 'Alt'
    },
    {
      id: 'MetaLeft',
      label: isMac ? <CommandIcon size={ICON} /> : <WindowsOutlined style={{ fontSize: ICON }} />,
      name: isMac ? 'Command' : 'Windows'
    }
  ];
}

const EDITING_KEYS: HelperKeyDef[] = [
  { id: 'Delete', label: 'Del', name: 'Delete' },
  { id: 'Escape', label: 'Esc', name: 'Escape' },
  { id: 'Tab', label: <ArrowRightToLineIcon size={ICON} />, name: 'Tab' },
  { id: 'Insert', label: 'Ins', name: 'Insert' }
];

const ENTER_KEYS: HelperKeyDef[] = [
  { id: 'Enter', label: <CornerDownLeftIcon size={ICON} />, name: 'Enter' }
];

const ARROW_KEYS: HelperKeyDef[] = [
  { id: 'ArrowLeft', label: <ArrowLeftIcon size={ICON} />, name: 'Left' },
  { id: 'ArrowUp', label: <ArrowUpIcon size={ICON} />, name: 'Up' },
  { id: 'ArrowDown', label: <ArrowDownIcon size={ICON} />, name: 'Down' },
  { id: 'ArrowRight', label: <ArrowRightIcon size={ICON} />, name: 'Right' }
];

const NAVIGATION_KEYS: HelperKeyDef[] = [
  { id: 'Home', label: <ChevronFirstIcon size={ICON} />, name: 'Home' },
  { id: 'End', label: <ChevronLastIcon size={ICON} />, name: 'End' },
  { id: 'PageUp', label: <ChevronsUpIcon size={ICON} />, name: 'Page Up' },
  { id: 'PageDown', label: <ChevronsDownIcon size={ICON} />, name: 'Page Down' }
];

const FUNCTION_KEYS: HelperKeyDef[] = Array.from({ length: 12 }, (_, index) => ({
  id: `F${index + 1}`,
  label: `F${index + 1}`
}));

// Keys a Mac keyboard does not have.
const PAUSE_KEYS: HelperKeyDef[] = [
  { id: 'Pause', label: 'Pause' },
  { id: 'Break', label: 'Break', combo: { modifiers: ['ControlLeft'], code: 'Pause' } }
];

function otherKeys(isMac: boolean): HelperKeyDef[] {
  const shortcut = isMac ? 'MetaLeft' : 'ControlLeft';
  const prefix = isMac ? 'Cmd' : 'Ctrl';
  const shortcuts = ['C', 'V', 'S'].map((letter) => ({
    id: `${prefix}+${letter}`,
    label: `${prefix}+${letter}`,
    combo: { modifiers: [shortcut], code: `Key${letter}` }
  }));
  if (isMac) return shortcuts;
  return [
    { id: 'PrintScreen', label: 'PrtScr', name: 'Print Screen' },
    { id: 'ScrollLock', label: 'ScrLk', name: 'Scroll Lock' },
    { id: 'ContextMenu', label: 'Menu' },
    ...shortcuts,
    {
      id: 'Ctrl+Alt+Del',
      label: 'Ctrl+Alt+Del',
      combo: { modifiers: ['ControlLeft', 'AltLeft'], code: 'Delete' }
    }
  ];
}

type HelperKeyProps = {
  id: string;
  label: ReactNode;
  name?: string;
  // Pressed (a modifier until the next key, an opened group).
  active?: boolean;
  // A modifier that stays pressed.
  locked?: boolean;
  onPress: () => void;
};

const HelperKey = ({ id, label, name, active, locked, onPress }: HelperKeyProps) => (
  <button
    type="button"
    tabIndex={-1}
    data-helper-key={id}
    data-locked={locked ? 'true' : undefined}
    aria-label={name}
    aria-pressed={active}
    className={clsx(
      'flex h-10 min-w-10 shrink-0 cursor-pointer select-none items-center justify-center whitespace-nowrap rounded border-0 px-1.5 text-xs',
      locked
        ? 'bg-accent text-[#fff]'
        : active
          ? 'bg-neutral-600 text-neutral-100'
          : 'bg-neutral-800 text-neutral-100 active:bg-neutral-700'
    )}
    onClick={onPress}
  >
    {label}
  </button>
);

type KeyHelperProps = {
  modes: ModifierModes;
  isMac: boolean;
  onModifier: (code: string) => void;
  onKey: (code: string) => void;
  onCombo: (modifiers: string[], code: string) => void;
  onClose: () => void;
};

export const KeyHelper = ({
  modes,
  isMac,
  onModifier,
  onKey,
  onCombo,
  onClose
}: KeyHelperProps) => {
  const { t } = useTranslation();
  const [showMore, setShowMore] = useState(true);
  const [showFunctionKeys, setShowFunctionKeys] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);
  const rowRef = useRef<HTMLDivElement | null>(null);
  // Which edges of the row have keys beyond them; those edges fade out.
  const [more, setMore] = useState({ left: false, right: false });
  const inset = useSystemKeyboardInset(true);
  const setHeight = useSetAtom(keyHelperHeightAtom);
  // The row and the system keyboard under it both cover the page.
  useReportHeight(ref, true, setHeight, inset);

  // The focus, and with it the system keyboard, stays on the hidden input
  // wherever the row is touched.
  const keepFocus = (event: SyntheticEvent) => event.preventDefault();
  // A mouse wheel moves the row sideways.
  const scrollSideways = (event: WheelEvent<HTMLDivElement>) => {
    if (event.deltaX === 0) event.currentTarget.scrollLeft += event.deltaY;
  };

  const updateEdges = useCallback(() => {
    const row = rowRef.current;
    if (!row) return;
    const left = row.scrollLeft > 1;
    const right = row.scrollLeft + row.clientWidth < row.scrollWidth - 1;
    setMore((current) =>
      current.left === left && current.right === right ? current : { left, right }
    );
  }, []);

  useEffect(() => {
    updateEdges();
    const row = rowRef.current;
    if (typeof ResizeObserver === 'undefined' || !row) return;
    const observer = new ResizeObserver(updateEdges);
    observer.observe(row);
    return () => observer.disconnect();
  }, [updateEdges, showMore, showFunctionKeys]);

  // Fn brings its keys into view: the row moves until Fn is at its start.
  function toggleFunctionKeys() {
    const next = !showFunctionKeys;
    setShowFunctionKeys(next);
    if (!next) return;
    requestAnimationFrame(() => {
      const row = rowRef.current;
      const button = row?.querySelector('[data-helper-key="fn"]');
      if (!row || !button) return;
      const offset = button.getBoundingClientRect().left - row.getBoundingClientRect().left;
      row.scrollTo({ left: row.scrollLeft + offset - 6, behavior: 'smooth' });
    });
  }

  const fade = `linear-gradient(to right, ${more.left ? 'transparent' : '#000'}, #000 ${EDGE_FADE_PX}px, #000 calc(100% - ${EDGE_FADE_PX}px), ${more.right ? 'transparent' : '#000'})`;
  const rowStyle: CSSProperties = {
    touchAction: 'pan-x',
    maskImage: fade,
    WebkitMaskImage: fade
  };

  const renderKeys = (keys: HelperKeyDef[]) =>
    keys.map((key) => (
      <HelperKey
        key={key.id}
        id={key.id}
        label={key.label}
        name={key.name}
        onPress={() => (key.combo ? onCombo(key.combo.modifiers, key.combo.code) : onKey(key.id))}
      />
    ));

  const groups: Array<[string, ReactNode]> = [
    [
      'modifiers',
      modifierKeys(isMac).map((key) => {
        const mode = modes[key.id] ?? 'off';
        return (
          <HelperKey
            key={key.id}
            id={key.id}
            label={key.label}
            name={key.name}
            active={mode !== 'off'}
            locked={mode === 'locked'}
            onPress={() => onModifier(key.id)}
          />
        );
      })
    ]
  ];
  if (showMore) {
    groups.push(
      ['editing', renderKeys(EDITING_KEYS)],
      ['enter', renderKeys(ENTER_KEYS)],
      ['arrows', renderKeys(ARROW_KEYS)],
      ['navigation', renderKeys(NAVIGATION_KEYS)]
    );
  }
  groups.push([
    'function-keys',
    <>
      <HelperKey
        id="fn"
        label="Fn"
        name={t('keyboard.helper.functionKeys')}
        active={showFunctionKeys}
        onPress={toggleFunctionKeys}
      />
      {showFunctionKeys && renderKeys(FUNCTION_KEYS)}
    </>
  ]);
  if (showMore && !isMac) groups.push(['pause-break', renderKeys(PAUSE_KEYS)]);
  groups.push(
    [
      'other',
      <>
        <HelperKey
          id="more"
          label={<EllipsisIcon size={ICON} />}
          name={t('keyboard.helper.more')}
          active={showMore}
          onPress={() => setShowMore(!showMore)}
        />
        {showMore && renderKeys(otherKeys(isMac))}
      </>
    ],
    [
      'close',
      <HelperKey
        id="close"
        label={<KeyboardOffIcon size={ICON} />}
        name={t('keyboard.helper.close')}
        onPress={onClose}
      />
    ]
  );

  return (
    <div
      ref={ref}
      role="toolbar"
      aria-label={t('keyboard.helper.title')}
      data-testid="key-helper"
      className="fixed inset-x-0 z-[1200] border-0 border-t border-solid border-neutral-700 bg-neutral-900 py-1.5"
      style={{ bottom: inset }}
      onPointerDown={keepFocus}
      onMouseDown={keepFocus}
    >
      <div
        ref={rowRef}
        data-testid="key-helper-row"
        className="flex gap-5 overflow-x-auto overscroll-x-contain px-1.5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        style={rowStyle}
        onWheel={scrollSideways}
        onScroll={updateEdges}
      >
        {groups.map(([name, keys]) => (
          <div key={name} data-helper-group={name} className="flex shrink-0 gap-1">
            {keys}
          </div>
        ))}
      </div>
      {/* More keys beyond an edge of the row. */}
      {more.left && (
        <div className="pointer-events-none absolute inset-y-0 left-0 flex items-center text-neutral-300">
          <ChevronLeftIcon size={14} />
        </div>
      )}
      {more.right && (
        <div className="pointer-events-none absolute inset-y-0 right-0 flex items-center text-neutral-300">
          <ChevronRightIcon size={14} />
        </div>
      )}
    </div>
  );
};
