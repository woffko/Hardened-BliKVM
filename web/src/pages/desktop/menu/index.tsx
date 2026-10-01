import { useEffect, useRef } from 'react';
import { Divider } from 'antd';
import clsx from 'clsx';
import { useAtomValue, useSetAtom } from 'jotai';
import { ChevronDownIcon, GripVerticalIcon } from 'lucide-react';
import Draggable, { DraggableData, DraggableEvent } from 'react-draggable';
import { useTranslation } from 'react-i18next';

import {
  isAvailable,
  supportsMenuItem,
  supportsRestrictedControls,
  type CapabilityMenuItem
} from '@/lib/capabilities.ts';
import { HARDENED_LOGO_SRC, HARDENED_NAME, HARDENED_SHORT_NAME } from '@/lib/hardened.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { menuBarOccupiesTopAtom, menuDisabledItemsAtom } from '@/jotai/settings.ts';
import { useMenuBounds } from '@/hooks/useMenuBounds.ts';
import { useMenuVisibility } from '@/hooks/useMenuVisibility.ts';
import { useIsDesktopLayout } from '@/hooks/useResponsiveLayout.ts';

import { DownloadImage } from './download.tsx';
import { Fullscreen } from './fullscreen';
import { Image } from './image';
import { Keyboard } from './keyboard';
import { LocalKeyboardButton } from './local-keyboard-button';
import { Mouse } from './mouse';
import { Collapse, Expand } from './operations';
import { Picoclaw } from './picoclaw';
import { Power } from './power';
import { RemoteControl } from './remote-control.tsx';
import { Screen } from './screen';
import { Script } from './script';
import { Settings } from './settings';
import { Terminal } from './terminal';
import { Wol } from './wol';

export const Menu = () => {
  const { t } = useTranslation();
  const nodeRef = useRef<HTMLDivElement | null>(null);
  const isDesktopLayout = useIsDesktopLayout();

  const capabilities = useAtomValue(capabilitiesAtom);
  const hidAvailable = Boolean(capabilities && isAvailable(capabilities.hid.status));
  const restrictedHid = supportsRestrictedControls(capabilities);
  const menuDisabledItems = useAtomValue(menuDisabledItemsAtom);

  const {
    isInitialized,
    isMenuExpanded,
    isMenuHidden,
    handleHovered,
    handleTouched,
    handleMoved,
    setIsMenuExpanded
  } = useMenuVisibility();

  const menuBounds = useMenuBounds(nodeRef, isMenuExpanded);
  const showExpandedMenu = isDesktopLayout ? isMenuExpanded : true;
  // Auto hide works on both layouts. Without a mouse, a touch on the bar
  // restarts its countdown and a tab at the top edge brings it back.
  const shouldAutoHide = isMenuHidden;

  // While the bar shows on the touch layout, the screen goes below it.
  const setMenuBarOccupiesTop = useSetAtom(menuBarOccupiesTopAtom);
  const occupiesTop = !isDesktopLayout && isInitialized && !shouldAutoHide;
  useEffect(() => {
    setMenuBarOccupiesTop(occupiesTop);
  }, [occupiesTop, setMenuBarOccupiesTop]);
  useEffect(() => () => setMenuBarOccupiesTop(false), [setMenuBarOccupiesTop]);

  function onDragStop(_e: DraggableEvent, data: DraggableData) {
    if (!isDesktopLayout) return;
    if (data.x === 0 && data.y === 0) return;
    handleMoved();
  }

  function isEnabled(item: CapabilityMenuItem) {
    return supportsMenuItem(capabilities, item) && !menuDisabledItems.includes(item);
  }

  return (
    <Draggable
      nodeRef={nodeRef}
      bounds={menuBounds}
      disabled={!isDesktopLayout}
      handle="strong"
      positionOffset={{ x: '-50%', y: '0%' }}
      onStop={onDragStop}
    >
      <div
        ref={nodeRef}
        className={clsx(
          'fixed left-1/2 top-[10px] z-[1000] -translate-x-1/2 transition-opacity duration-300',
          isInitialized ? 'opacity-100' : 'opacity-0'
        )}
        // A tap also fires mouse events, and the mouse never leaves on a
        // touch screen: there, touches drive the countdown instead.
        onMouseEnter={isDesktopLayout ? () => handleHovered(true) : undefined}
        onMouseLeave={isDesktopLayout ? () => handleHovered(false) : undefined}
        onBlur={isDesktopLayout ? () => handleHovered(false) : undefined}
        onPointerDownCapture={isDesktopLayout ? undefined : handleTouched}
      >
        {/* Touch layout: the tab that brings the hidden bar back */}
        {!isDesktopLayout && shouldAutoHide && (
          <button
            type="button"
            data-testid="menu-reveal"
            aria-label={t('menu.show')}
            className="absolute -top-[10px] left-1/2 z-10 flex h-[22px] w-[52px] -translate-x-1/2 items-center justify-center rounded-b bg-neutral-800/80 text-neutral-300"
            onClick={handleTouched}
          >
            <ChevronDownIcon size={16} />
          </button>
        )}

        {/* Trigger area for auto-show when hidden */}
        {isDesktopLayout && isMenuExpanded && (
          <div className="absolute -top-[10px] left-0 right-0 h-[46px] w-full bg-transparent" />
        )}

        {/* Menubar */}
        <div
          className={clsx(
            'sticky top-[10px] flex w-full justify-center',
            // Hidden on a touch screen, the bar must not take the touches
            // meant for the remote screen or for the tab above.
            !isDesktopLayout && shouldAutoHide && 'pointer-events-none'
          )}
        >
          <div
            data-testid="menu-bar"
            data-hidden={shouldAutoHide ? 'true' : 'false'}
            className={clsx(
              'h-[36px] max-w-[calc(100vw-12px)] items-center overflow-x-auto rounded bg-neutral-800/80 pl-1 pr-2 transition-all duration-300',
              showExpandedMenu ? 'flex' : 'hidden',
              shouldAutoHide ? '-translate-y-[110%] opacity-80' : 'translate-y-0 opacity-100'
            )}
          >
            <strong className={isDesktopLayout ? '' : 'hidden'}>
              <div className="flex h-[30px] cursor-move select-none items-center justify-center pl-1 text-neutral-500">
                <GripVerticalIcon size={18} />
              </div>
            </strong>
            {isDesktopLayout && <Divider type="vertical" />}

            <div
              className="flex h-[30px] w-[32px] select-none items-center justify-start overflow-hidden rounded bg-[#fff] sm:w-[98px] sm:justify-center"
              title={HARDENED_NAME}
            >
              <img
                src={HARDENED_LOGO_SRC}
                alt={HARDENED_SHORT_NAME}
                className="h-auto w-[98px] max-w-none shrink-0"
              />
            </div>
            <Divider type="vertical" />

            <Screen />
            {restrictedHid && <RemoteControl />}
            {hidAvailable && <LocalKeyboardButton />}
            {hidAvailable && <Keyboard />}
            {hidAvailable && <Mouse />}
            <Divider type="vertical" />

            {isEnabled('image') && <Image />}
            {isEnabled('download') && <DownloadImage />}
            {isEnabled('terminal') && <Terminal />}
            {isEnabled('script') && <Script />}
            {isEnabled('wol') && <Wol />}

            {(['image', 'download', 'script', 'terminal', 'wol'] as const).some(isEnabled) && (
              <Divider type="vertical" />
            )}

            {isEnabled('picoclaw') && (
              <>
                <Picoclaw />
                <Divider type="vertical" />
              </>
            )}

            {isEnabled('power') && (
              <>
                <Power />
                <Divider type="vertical" />
              </>
            )}

            <Settings />
            {isEnabled('fullscreen') && <Fullscreen />}
            {isDesktopLayout && isEnabled('collapse') && (
              <Collapse toggleMenu={setIsMenuExpanded} />
            )}
          </div>
        </div>

        {/* Menubar expand button */}
        {isDesktopLayout && !isMenuExpanded && <Expand toggleMenu={setIsMenuExpanded} />}
      </div>
    </Draggable>
  );
};
