import { useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { useSetAtom } from 'jotai';
import {
  ArrowDownIcon,
  ArrowLeftIcon,
  ArrowRightIcon,
  ArrowUpIcon,
  EllipsisIcon,
  KeyboardOffIcon
} from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { keyHelperHeightAtom } from '@/jotai/keyboard.ts';
import { useReportHeight, useSystemKeyboardInset } from '@/hooks/useKeyboardInsets.ts';

// Keys a phone keyboard lacks, docked above the system keyboard while the
// mobile keyboard is open. A modifier is armed by a tap and goes with the
// next key, from this row or typed on the system keyboard.

const HELPER_MODIFIERS = [
  { code: 'ControlLeft', label: 'Ctrl' },
  { code: 'AltLeft', label: 'Alt' },
  { code: 'ShiftLeft', label: 'Shift' },
  { code: 'MetaLeft', label: 'Win' }
];

type HelperKeyDef = { code: string; label: ReactNode };

const EDIT_KEYS: HelperKeyDef[] = [
  { code: 'Escape', label: 'Esc' },
  { code: 'Tab', label: 'Tab' }
];

const ARROW_KEYS: HelperKeyDef[] = [
  { code: 'ArrowLeft', label: <ArrowLeftIcon size={16} /> },
  { code: 'ArrowUp', label: <ArrowUpIcon size={16} /> },
  { code: 'ArrowDown', label: <ArrowDownIcon size={16} /> },
  { code: 'ArrowRight', label: <ArrowRightIcon size={16} /> }
];

const FUNCTION_KEYS: HelperKeyDef[] = Array.from({ length: 12 }, (_, index) => ({
  code: `F${index + 1}`,
  label: `F${index + 1}`
}));

const MORE_KEYS: HelperKeyDef[] = [
  { code: 'Home', label: 'Home' },
  { code: 'End', label: 'End' },
  { code: 'PageUp', label: 'PgUp' },
  { code: 'PageDown', label: 'PgDn' },
  { code: 'Insert', label: 'Ins' },
  { code: 'Delete', label: 'Del' },
  { code: 'PrintScreen', label: 'PrtSc' },
  { code: 'Pause', label: 'Pause' }
];

type HelperKeyProps = {
  id: string;
  label: ReactNode;
  title?: string;
  active?: boolean;
  onPress: () => void;
};

const HelperKey = ({ id, label, title, active, onPress }: HelperKeyProps) => (
  <button
    type="button"
    tabIndex={-1}
    data-helper-key={id}
    aria-label={title}
    aria-pressed={active}
    className={clsx(
      'flex h-9 min-w-[42px] select-none items-center justify-center rounded border border-solid px-2 text-sm',
      active
        ? 'border-accent bg-accent text-[#fff]'
        : 'border-neutral-700 bg-neutral-800 text-neutral-100 active:bg-neutral-700'
    )}
    // The focus, and with it the system keyboard, stays on the hidden input.
    onPointerDown={(event) => event.preventDefault()}
    onMouseDown={(event) => event.preventDefault()}
    onClick={onPress}
  >
    {label}
  </button>
);

type KeyHelperProps = {
  armed: string[];
  onToggleModifier: (code: string) => void;
  onKey: (code: string) => void;
  onCombo: (codes: string[], code: string) => void;
  onClose: () => void;
};

export const KeyHelper = ({ armed, onToggleModifier, onKey, onCombo, onClose }: KeyHelperProps) => {
  const { t } = useTranslation();
  const [panel, setPanel] = useState<'none' | 'fn' | 'more'>('none');
  const ref = useRef<HTMLDivElement | null>(null);
  const inset = useSystemKeyboardInset(true);
  const setHeight = useSetAtom(keyHelperHeightAtom);
  // The row and the system keyboard under it both cover the page.
  useReportHeight(ref, true, setHeight, inset);

  const togglePanel = (next: 'fn' | 'more') => setPanel(panel === next ? 'none' : next);
  const renderKeys = (keys: HelperKeyDef[]) =>
    keys.map((key) => (
      <HelperKey key={key.code} id={key.code} label={key.label} onPress={() => onKey(key.code)} />
    ));

  return (
    <div
      ref={ref}
      role="toolbar"
      aria-label={t('keyboard.helper.title')}
      data-testid="key-helper"
      className="fixed inset-x-0 z-[1200] border-t border-neutral-700 bg-neutral-900/95 p-1.5 shadow-2xl backdrop-blur"
      style={{ bottom: inset }}
    >
      <div className="flex flex-wrap gap-1">
        {HELPER_MODIFIERS.map((modifier) => (
          <HelperKey
            key={modifier.code}
            id={modifier.code}
            label={modifier.label}
            active={armed.includes(modifier.code)}
            onPress={() => onToggleModifier(modifier.code)}
          />
        ))}
        {renderKeys(EDIT_KEYS)}
        {/* The arrows stay together when the row wraps. */}
        <div className="flex gap-1">{renderKeys(ARROW_KEYS)}</div>
        <HelperKey
          id="fn"
          label="Fn"
          title={t('keyboard.helper.functionKeys')}
          active={panel === 'fn'}
          onPress={() => togglePanel('fn')}
        />
        <HelperKey
          id="more"
          label={<EllipsisIcon size={16} />}
          title={t('keyboard.helper.more')}
          active={panel === 'more'}
          onPress={() => togglePanel('more')}
        />
        <HelperKey
          id="close"
          label={<KeyboardOffIcon size={16} />}
          title={t('keyboard.helper.close')}
          onPress={onClose}
        />
      </div>
      {panel === 'fn' && (
        <div className="mt-1 flex flex-wrap gap-1">{renderKeys(FUNCTION_KEYS)}</div>
      )}
      {panel === 'more' && (
        <div className="mt-1 flex flex-wrap gap-1">
          {renderKeys(MORE_KEYS)}
          <HelperKey
            id="ctrl-alt-del"
            label="Ctrl+Alt+Del"
            onPress={() => onCombo(['ControlLeft', 'AltLeft'], 'Delete')}
          />
        </div>
      )}
    </div>
  );
};
