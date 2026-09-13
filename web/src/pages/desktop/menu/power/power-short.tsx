import { Popconfirm } from 'antd';
import { PowerIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/vm.ts';
import { useRestrictedPower } from '@/hooks/useRestrictedPower.ts';

type PowerShortProps = {
  showConfirm: boolean;
  isLoading: boolean;
  setIsLoading: (loading: boolean) => void;
};

export const PowerShort = ({ showConfirm, isLoading, setIsLoading }: PowerShortProps) => {
  const { t } = useTranslation();
  const { restricted, run } = useRestrictedPower();

  function power() {
    if (isLoading) return;
    setIsLoading(true);

    const finished = () => setIsLoading(false);
    if (restricted) {
      void run('power_short').finally(finished);
    } else {
      api.setGpio('power', 800).finally(finished);
    }
  }

  return (
    <>
      {showConfirm ? (
        <Popconfirm
          placement="bottomLeft"
          title={t('power.powerConfirm')}
          okText={t('power.okBtn')}
          cancelText={t('power.cancelBtn')}
          onConfirm={power}
          color="#404040"
        >
          <div className="flex cursor-pointer select-none items-center space-x-2 rounded px-3 py-1.5 hover:bg-neutral-700/70">
            <PowerIcon size={16} />
            <span>{t('power.powerShort')}</span>
          </div>
        </Popconfirm>
      ) : (
        <div
          className="flex cursor-pointer select-none items-center space-x-2 rounded px-3 py-1.5 hover:bg-neutral-700/70"
          onClick={power}
        >
          <PowerIcon size={16} />
          <span>{t('power.powerShort')}</span>
        </div>
      )}
    </>
  );
};
