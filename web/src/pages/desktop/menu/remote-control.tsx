import { Button, Tooltip } from 'antd';
import { ShieldCheckIcon, ShieldIcon } from 'lucide-react';

import { useFocusConfirmation } from '@/hooks/useFocusConfirmation.ts';

export const RemoteControl = () => {
  const { confirmed, requestConfirmation, disableControl } = useFocusConfirmation();

  return (
    <Tooltip title={confirmed ? 'Disable remote control' : 'Enable remote control'}>
      <Button
        type="text"
        size="small"
        aria-pressed={confirmed}
        aria-label={confirmed ? 'Disable remote control' : 'Enable remote control'}
        className={confirmed ? 'text-emerald-400' : 'text-neutral-300'}
        icon={confirmed ? <ShieldCheckIcon size={18} /> : <ShieldIcon size={18} />}
        onClick={() => {
          if (confirmed) {
            disableControl();
          } else {
            void requestConfirmation();
          }
        }}
      />
    </Tooltip>
  );
};
