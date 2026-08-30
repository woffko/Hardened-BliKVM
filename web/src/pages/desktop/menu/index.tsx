import { useRef } from 'react';
import { Divider } from 'antd';
import clsx from 'clsx';
import { useAtomValue } from 'jotai';
import { GripVerticalIcon } from 'lucide-react';
import Draggable, { DraggableData, DraggableEvent } from 'react-draggable';

import { isAvailable, supportsMenuItem, type CapabilityMenuItem } from '@/lib/capabilities.ts';
import { HARDENED_LOGO_SRC, HARDENED_NAME, HARDENED_SHORT_NAME } from '@/lib/hardened.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { menuDisabledItemsAtom } from '@/jotai/settings.ts';
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
import { Screen } from './screen';
import { Script } from './script';
import { Settings } from './settings';
import { Terminal } from './terminal';
import { Wol } from './wol';

export const Menu = () => {
  const nodeRef = useRef<HTMLDivElement | null>(null);
  const isDesktopLayout = useIsDesktopLayout();

  const capabilities = useAtomValue(capabilitiesAtom);
  const hidAvailable = Boolean(capabilities && isAvailable(capabilities.hid.status));
  const menuDisabledItems = useAtomValue(menuDisabledItemsAtom);

  const {
    isInitialized,
    isMenuExpanded,
    isMenuHidden,
    handleHovered,
    handleMoved,
    setIsMenuExpanded
  } = useMenuVisibility();

  const menuBounds = useMenuBounds(nodeRef, isMenuExpanded);
  const showExpandedMenu = isDesktopLayout ? isMenuExpanded : true;
  const shouldAutoHide = isDesktopLayout && isMenuHidden;

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
        onMouseEnter={() => handleHovered(true)}
        onMouseLeave={() => handleHovered(false)}
        onBlur={() => handleHovered(false)}
      >
        {/* Trigger area for auto-show when hidden */}
        {isDesktopLayout && isMenuExpanded && (
          <div className="absolute -top-[10px] left-0 right-0 h-[46px] w-full bg-transparent" />
        )}

        {/* Menubar */}
        <div className="sticky top-[10px] flex w-full justify-center">
          <div
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
              className="flex h-[30px] w-[32px] select-none items-center justify-start overflow-hidden rounded bg-white sm:w-[98px] sm:justify-center"
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
