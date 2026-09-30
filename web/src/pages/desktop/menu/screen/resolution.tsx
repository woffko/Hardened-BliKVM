import { useRef } from 'react';
import { message, Tooltip } from 'antd';
import { CheckIcon, CircleHelpIcon, RatioIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { updateScreen } from '@/api/vm';
import { Resolution as TypeResolution } from '@/types';
import { MenuSubmenu } from '@/components/menu-submenu.tsx';

import { ResolutionList } from './constants.ts';

// The reviewed unit always passes a resolution, so there is no "auto" entry;
// the four modes below are the ones the owner can enforce.
export const Resolution = ({
  resolution,
  refresh
}: {
  resolution: TypeResolution;
  refresh: () => Promise<{ requestedResolution?: string }>;
}) => {
  const { t } = useTranslation();
  const updating = useRef(false);

  async function update(item: TypeResolution) {
    if (updating.current) return;
    updating.current = true;
    try {
      const rsp = await updateScreen('resolution', item.height);
      if (rsp.code !== 0 || rsp.data?.applied !== true || rsp.data?.rejection != null) {
        message.error(rsp.data?.rejection || rsp.msg || 'Failed to update resolution');
        return;
      }

      const observed = await refresh();
      if (observed.requestedResolution !== `${item.width}x${item.height}`)
        throw new Error('Resolution read-back did not match the request');
    } catch (err) {
      message.error(err instanceof Error ? err.message : 'Failed to update resolution');
    } finally {
      updating.current = false;
    }
  }

  const content = (
    <>
      {ResolutionList.map((res) => (
        <div
          key={res.height}
          className="flex cursor-pointer select-none items-center rounded py-1.5 pl-1 pr-5 hover:bg-neutral-700/70"
          onClick={() => update(res)}
        >
          <div className="flex h-[14px] w-[20px] items-end text-blue-500">
            {res.height === resolution?.height && <CheckIcon size={15} />}
          </div>

          {res.height === 0 ? (
            <div className="flex items-center justify-between space-x-2">
              <span>{t('screen.auto')}</span>
              <Tooltip
                title={t('screen.autoTips')}
                placement="right"
                overlayInnerStyle={{ width: '300px' }}
              >
                <CircleHelpIcon size={14} />
              </Tooltip>
            </div>
          ) : (
            <>
              <span>{res.width}</span>
              <span className="px-1">x</span>
              <span>{res.height}</span>
            </>
          )}
        </div>
      ))}
    </>
  );

  return (
    <MenuSubmenu icon={<RatioIcon size={18} />} label={t('screen.resolution')} content={content} />
  );
};
