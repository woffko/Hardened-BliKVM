import { useEffect } from 'react';
import { Button, notification } from 'antd';
import { useTranslation } from 'react-i18next';
import { useNavigate } from 'react-router-dom';

import { isPasswordUpdated } from '@/api/auth.ts';
import { getSkipModifyPassword, setSkipModifyPassword } from '@/lib/localstorage.ts';

export const Notification = () => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [api, contextHolder] = notification.useNotification();

  useEffect(() => {
    const skip = getSkipModifyPassword();
    if (skip) return;

    let cancelled = false;
    isPasswordUpdated()
      .then((rsp) => {
        if (!cancelled && rsp.code === 0 && !rsp.data.isUpdated) {
          api.warning({
            key: 'no_change_password',
            message: t('auth.changePassword'),
            description: t('auth.changePasswordDesc'),
            placement: 'topRight',
            btn: (
              <Button
                type="primary"
                onClick={() => {
                  api.destroy();
                  navigate('/auth/password');
                }}
              >
                {t('auth.ok')}
              </Button>
            ),
            duration: null,
            onClose: () => setSkipModifyPassword(true)
          });
        }
      })
      .catch(() => {
        // Session expiry/logout is handled by the HTTP interceptor. A failed
        // reminder check must not become an unhandled rejection or mark the
        // password as updated/skipped.
      });
    return () => {
      cancelled = true;
    };
  }, [api, navigate, t]);

  return <>{contextHolder}</>;
};
