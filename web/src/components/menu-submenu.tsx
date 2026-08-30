import { ReactNode, useState } from 'react';
import { Popover } from 'antd';
import clsx from 'clsx';
import { ChevronRightIcon } from 'lucide-react';

import { useIsDesktopLayout } from '@/hooks/useResponsiveLayout.ts';

type MenuSubmenuProps = {
  icon: ReactNode;
  label: ReactNode;
  content: ReactNode;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
};

export const MenuSubmenu = ({ icon, label, content, open, onOpenChange }: MenuSubmenuProps) => {
  const isDesktopLayout = useIsDesktopLayout();
  const [internalOpen, setInternalOpen] = useState(false);
  const isOpen = open ?? internalOpen;

  function setOpen(nextOpen: boolean) {
    if (open === undefined) {
      setInternalOpen(nextOpen);
    }
    onOpenChange?.(nextOpen);
  }

  const trigger = (
    <div
      data-menu-keep-open
      className="flex min-h-[30px] cursor-pointer items-center rounded px-3 py-1 text-neutral-300 hover:bg-neutral-700/70"
      onClick={isDesktopLayout ? undefined : () => setOpen(!isOpen)}
    >
      <div className="mr-2 flex h-[18px] w-[18px] shrink-0 items-center justify-center">{icon}</div>
      <span className="min-w-0 flex-1 select-none text-sm">{label}</span>
      {!isDesktopLayout && (
        <ChevronRightIcon
          size={16}
          className={clsx('ml-2 shrink-0 transition-transform', isOpen && 'rotate-90')}
        />
      )}
    </div>
  );

  if (isDesktopLayout) {
    return (
      <Popover
        content={content}
        placement="rightTop"
        arrow={false}
        align={{ offset: [14, 0] }}
        open={open === undefined ? undefined : isOpen}
        onOpenChange={setOpen}
      >
        {trigger}
      </Popover>
    );
  }

  return (
    <div className="min-w-0">
      {trigger}
      {isOpen && (
        <div className="ml-5 mt-1 max-w-full overflow-x-auto border-l border-neutral-700 pl-2">
          {content}
        </div>
      )}
    </div>
  );
};
