import { MouseEvent, ReactNode, useState } from 'react';
import { Popover, Tooltip } from 'antd';
import { useSetAtom } from 'jotai';

import { submenuOpenCountAtom } from '@/jotai/settings.ts';
import { useIsDesktopLayout } from '@/hooks/useResponsiveLayout.ts';

type MenuItemProps = {
  title: string;
  icon: ReactNode;
  content: ReactNode;
  className?: string;
  fresh?: boolean;
  onOpenChange?: (open: boolean) => void;
};

export const MenuItem = ({
  title,
  icon,
  content,
  className,
  fresh,
  onOpenChange
}: MenuItemProps) => {
  const isDesktopLayout = useIsDesktopLayout();
  const setSubmenuOpenCount = useSetAtom(submenuOpenCountAtom);

  const [isPopoverOpen, setIsPopoverOpen] = useState(false);
  const [isTooltipOpen, setIsTooltipOpen] = useState(false);
  const popoverContent = isDesktopLayout ? (
    content
  ) : (
    <div
      className="max-h-[calc(100dvh-72px)] w-[min(360px,calc(100vw-16px))] overflow-y-auto"
      onClickCapture={closeMobilePopoverAfterSelection}
    >
      {content}
    </div>
  );

  function togglePopover(open: boolean) {
    setIsTooltipOpen(false);
    setIsPopoverOpen(open);

    // Update global submenu count
    setSubmenuOpenCount((count) => (open ? count + 1 : Math.max(0, count - 1)));

    if (onOpenChange) {
      onOpenChange(open);
    }
  }

  function toggleTooltip(open: boolean) {
    if (isPopoverOpen) {
      return;
    }
    setIsTooltipOpen(open);
  }

  function closeMobilePopoverAfterSelection(event: MouseEvent<HTMLDivElement>) {
    if (isDesktopLayout) return;

    const target = event.target;
    if (!(target instanceof HTMLElement)) return;
    if (
      target.closest(
        [
          '[data-menu-keep-open]',
          '.ant-slider',
          '.ant-switch',
          '.ant-select',
          '.ant-input',
          'button',
          'input',
          'textarea',
          'select'
        ].join(',')
      )
    ) {
      return;
    }

    window.setTimeout(() => togglePopover(false), 0);
  }

  return (
    <Popover
      content={popoverContent}
      arrow={false}
      trigger="click"
      placement={isDesktopLayout ? 'bottomLeft' : 'bottom'}
      open={isPopoverOpen}
      onOpenChange={togglePopover}
      fresh={!!fresh}
      overlayStyle={isDesktopLayout ? undefined : { maxWidth: 'calc(100vw - 8px)' }}
    >
      <Tooltip
        title={isDesktopLayout ? title : undefined}
        mouseEnterDelay={0.6}
        placement="bottom"
        open={isDesktopLayout ? isTooltipOpen : false}
        onOpenChange={toggleTooltip}
      >
        <div
          className={
            className
              ? className
              : 'flex h-[30px] w-[30px] cursor-pointer items-center justify-center rounded text-neutral-300 hover:bg-neutral-700/80 hover:text-white'
          }
        >
          {icon}
        </div>
      </Tooltip>
    </Popover>
  );
};
