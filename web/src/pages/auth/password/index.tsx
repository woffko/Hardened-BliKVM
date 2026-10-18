import { useEffect, useState } from 'react';
import { LockOutlined, UserOutlined } from '@ant-design/icons';
import { Button, Card, Form, Input } from 'antd';
import { useTranslation } from 'react-i18next';
import { useNavigate } from 'react-router-dom';

import * as api from '@/api/auth.ts';
import { removeToken } from '@/lib/cookie.ts';
import { encrypt } from '@/lib/encrypt.ts';
import { Head } from '@/components/head.tsx';

export const Password = () => {
  const { t } = useTranslation();
  const [msg, setMsg] = useState('');
  const navigate = useNavigate();
  const [form] = Form.useForm();

  useEffect(() => {
    api
      .getAccount()
      .then((response: any) => {
        if (response.code === 0 && typeof response.data?.username === 'string') {
          form.setFieldValue('username', response.data.username);
        }
      })
      .catch(() => setMsg(t('auth.error')));
  }, [form, t]);

  useEffect(() => {
    if (msg) {
      setTimeout(() => setMsg(''), 3000);
    }
  }, [msg]);

  function changePassword(values: any) {
    if (values.password !== values.password2) {
      setMsg(t('auth.differentPassword'));
      return;
    }
    if (!validateString(values.username)) {
      setMsg(t('auth.illegalUsername'));
      return;
    }
    if (values.password.length < 8 || !validateString(values.password)) {
      setMsg(t('auth.illegalPassword'));
      return;
    }

    const username = values.username;
    const password = encrypt(values.password);
    const currentPassword = encrypt(values.currentPassword);

    api
      .changePassword(username, password, currentPassword)
      .then((rsp: any) => {
        if (rsp.code !== 0) {
          setMsg(t(rsp.code === -403 ? 'auth.wrongCurrentPassword' : 'auth.error'));
          return;
        }

        removeToken();
        navigate('/auth/login', { replace: true });
      })
      .catch((error: any) => {
        const status = error?.response?.status;
        setMsg(
          t(
            status === 403
              ? 'auth.wrongCurrentPassword'
              : status === 429
                ? 'auth.locked'
                : 'auth.error'
          )
        );
      });
  }

  function validateString(str: string) {
    const regex = /['"\\/]/;
    return !regex.test(str);
  }

  function cancel() {
    window.location.replace('/');
  }

  return (
    <>
      <Head title={t('head.changePassword')} />

      <div className="flex h-screen w-screen flex-col items-center justify-center space-y-5">
        <h2 className="text-xl font-semibold text-neutral-100">{t('auth.changePassword')}</h2>

        <Form form={form} style={{ minWidth: 300, maxWidth: 500 }} onFinish={changePassword}>
          <Form.Item
            name="username"
            rules={[{ required: true, message: t('auth.noEmptyUsername'), min: 1 }]}
          >
            <Input disabled prefix={<UserOutlined />} placeholder={t('auth.placeholderUsername')} />
          </Form.Item>

          <Form.Item
            name="currentPassword"
            rules={[{ required: true, message: t('auth.noEmptyPassword') }]}
          >
            <Input
              prefix={<LockOutlined />}
              type="password"
              autoComplete="current-password"
              placeholder={t('auth.placeholderCurrentPassword')}
            />
          </Form.Item>

          <Form.Item
            name="password"
            rules={[{ required: true, message: t('auth.noEmptyPassword'), min: 8 }]}
          >
            <Input
              prefix={<LockOutlined />}
              type="password"
              placeholder={t('auth.placeholderPassword')}
            />
          </Form.Item>

          <Form.Item
            name="password2"
            rules={[{ required: true, message: t('auth.noEmptyPassword'), min: 8 }]}
          >
            <Input
              prefix={<LockOutlined />}
              type="password"
              placeholder={t('auth.placeholderPassword2')}
            />
          </Form.Item>

          <span className="text-red-500">{msg}</span>
          <Form.Item>
            <div className="flex w-full space-x-2">
              <Button type="primary" htmlType="submit" className="w-1/2">
                {t('auth.ok')}
              </Button>
              <Button className="w-1/2" onClick={cancel}>
                {t('auth.cancel')}
              </Button>
            </div>
          </Form.Item>
        </Form>

        <Card>
          <div className="flex w-full max-w-[450px] flex-col">
            <div>{t('auth.tips.change1')}</div>
            <ul className="list-outside list-decimal">
              <li>{t('auth.tips.change2')}</li>
              <li>{t('auth.tips.change3')}</li>
            </ul>
            <div className="text-red-500">{t('auth.tips.change4')}</div>
          </div>
        </Card>
      </div>
    </>
  );
};
