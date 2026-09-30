import { useState } from 'react';
import type { CollapseProps } from 'antd';
import { Collapse, Modal } from 'antd';
import { CircleHelpIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

/** How images reach the store. The hardened board keeps a catalog, so an image
 * arrives only through the Download menu (a file or, when switched on, a URL). */
export const Tips = ({ hardened = false }: { hardened?: boolean }) => {
  const { t } = useTranslation();
  const [isModalOpen, setIsModalOpen] = useState(false);

  const hardenedItems: CollapseProps['items'] = [
    {
      key: 'upload',
      label: t('image.tips.uploadTitle'),
      children: (
        <ul className="list-decimal">
          <li>{t('image.tips.upload1')}</li>
          <li>{t('image.tips.upload2')}</li>
          <li>{t('image.tips.upload3')}</li>
          <li>{t('image.tips.upload4')}</li>
        </ul>
      )
    }
  ];

  const items: CollapseProps['items'] = [
    {
      key: '1',
      label: 'USB',
      children: (
        <ul className="list-decimal">
          <li>{t('image.tips.usb1')}</li>
          <li>{t('image.tips.usb2')}</li>
          <li>{t('image.tips.usb3')}</li>
        </ul>
      )
    },
    {
      key: '2',
      label: 'SCP',
      children: (
        <ul className="list-decimal">
          <li>{t('image.tips.scp1')}</li>
          <li>{t('image.tips.scp2')}</li>
          <li>{t('image.tips.scp3')}</li>
        </ul>
      )
    },
    {
      key: '3',
      label: t('image.tips.tfCard'),
      children: (
        <>
          <p className="pl-5 text-sm text-neutral-400">{t('image.tips.tf1')}</p>
          <ul className="list-decimal">
            <li>{t('image.tips.tf2')}</li>
            <li>{t('image.tips.tf3')}</li>
            <li>{t('image.tips.tf4')}</li>
            <li>{t('image.tips.tf5')}</li>
          </ul>
        </>
      )
    }
  ];

  return (
    <>
      <div
        className="flex cursor-pointer items-center space-x-1 text-neutral-500 hover:text-blue-500"
        onClick={() => setIsModalOpen(true)}
      >
        <CircleHelpIcon size={16} />
      </div>

      <Modal
        title={t('image.tips.title')}
        open={isModalOpen}
        width={520}
        footer={null}
        centered
        onCancel={() => setIsModalOpen(false)}
      >
        <Collapse
          accordion
          items={hardened ? hardenedItems : items}
          bordered={false}
          defaultActiveKey={[hardened ? 'upload' : '1']}
          style={{ backgroundColor: 'transparent' }}
        />
      </Modal>
    </>
  );
};
