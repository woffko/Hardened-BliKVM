import { Popconfirm, Tooltip } from 'antd';
import { LogOutIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useNavigate } from 'react-router-dom';

import { logOut } from '@/lib/logout.ts';

/**
 * Logout on the toolbar, after Settings: it asks as Settings > Account does
 * (docs/baselines/work-plan-20261117-system-log.md, L5).
 */
export const Logout = () => {
  const { t } = useTranslation();
  const navigate = useNavigate();

  return (
    <Popconfirm
      placement="bottom"
      title={t('settings.account.logoutDesc')}
      okText={t('settings.account.okBtn')}
      cancelText={t('settings.account.cancelBtn')}
      onConfirm={() => void logOut(navigate)}
    >
      <Tooltip title={t('menu.logout')} placement="bottom" mouseEnterDelay={0.6}>
        <div
          data-testid="toolbar-logout"
          className="flex h-[30px] w-[30px] cursor-pointer items-center justify-center rounded text-neutral-300 hover:bg-neutral-700/80 hover:text-white"
        >
          <LogOutIcon size={18} />
        </div>
      </Tooltip>
    </Popconfirm>
  );
};
