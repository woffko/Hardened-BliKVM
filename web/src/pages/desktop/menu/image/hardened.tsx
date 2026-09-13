import { useEffect, useState } from 'react';
import { Button, message, Modal, Radio, Segmented, Switch, Tooltip } from 'antd';
import clsx from 'clsx';
import { useAtomValue, useSetAtom } from 'jotai';
import { DiscIcon, HardDriveIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import {
  getMediaCatalog,
  submitVirtualMedia,
  type MediaImage,
  type VirtualMediaMode
} from '@/api/virtual-media.ts';
import { isWritable } from '@/lib/capabilities.ts';
import { newOperationId } from '@/lib/hid-lease.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { submenuOpenCountAtom } from '@/jotai/settings.ts';

import { Tips } from './tips.tsx';

const modeFor = (value: string): VirtualMediaMode =>
  value === 'cd-rom' ? 'cd_rom' : 'removable_disk';

/**
 * Hardened virtual-media panel. It lists the opaque catalog IDs and submits
 * typed select/set_connected commands; no path or filename is ever sent.
 */
export const HardenedImage = () => {
  const { t } = useTranslation();
  const manifest = useAtomValue(capabilitiesAtom);
  const writable = Boolean(manifest && isWritable(manifest.virtualMedia.status));
  const setSubmenuOpenCount = useSetAtom(submenuOpenCountAtom);

  const [isModalOpen, setIsModalOpen] = useState(false);
  const [images, setImages] = useState<MediaImage[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [mode, setMode] = useState('mass-storage');
  const [isConnected, setIsConnected] = useState(false);
  const [isLoading, setIsLoading] = useState(false);

  useEffect(() => {
    if (!isModalOpen) return;
    getMediaCatalog()
      .then((rsp) => {
        if (rsp.code === 0) {
          setImages(rsp.data.images);
          if (rsp.data.images.length > 0) {
            setSelected((current) => current ?? rsp.data.images[0].imageId);
          }
        }
      })
      .catch(() => undefined);
  }, [isModalOpen]);

  function toggleModal(open: boolean) {
    setIsModalOpen(open);
    setSubmenuOpenCount((count) => (open ? count + 1 : Math.max(0, count - 1)));
  }

  async function run(command: Parameters<typeof submitVirtualMedia>[1]) {
    setIsLoading(true);
    try {
      const rsp = await submitVirtualMedia(newOperationId(), command);
      if (rsp.data.applied) {
        message.success(t('image.mounted') || 'Virtual media command applied');
        return true;
      }
      message.error(rsp.data.rejection || 'Virtual media command rejected');
      return false;
    } catch {
      message.error('Virtual media command failed');
      return false;
    } finally {
      setIsLoading(false);
    }
  }

  async function mount() {
    if (selected === null) return;
    await run({ action: 'select', imageId: selected, mode: modeFor(mode), writable: false });
  }

  async function toggleConnection(next: boolean) {
    const applied = await run({ action: 'set_connected', connected: next });
    if (applied) {
      setIsConnected(next);
    }
  }

  const available = images.filter((image) =>
    mode === 'cd-rom' ? image.cdromAllowed : image.removableAllowed
  );

  return (
    <>
      <Tooltip title={t('image.title')} placement="bottom" mouseEnterDelay={0.6}>
        <div
          className={clsx(
            'flex h-[30px] w-[30px] cursor-pointer items-center justify-center rounded hover:bg-neutral-700',
            isConnected ? 'text-blue-500' : 'text-neutral-300 hover:text-white'
          )}
          onClick={() => toggleModal(true)}
        >
          <DiscIcon size={18} />
        </div>
      </Tooltip>

      <Modal open={isModalOpen} footer={null} onCancel={() => toggleModal(false)}>
        <div className="flex items-center space-x-1">
          <span className="text-xl font-bold">{t('image.title')}</span>
          <Tips />
        </div>

        <Segmented
          className="mt-4"
          value={mode}
          onChange={(value) => setMode(String(value))}
          options={[
            {
              value: 'mass-storage',
              label: (
                <div className="flex items-center space-x-1">
                  <HardDriveIcon size={16} />
                  <span>Mass Storage</span>
                </div>
              )
            },
            {
              value: 'cd-rom',
              label: (
                <div className="flex items-center space-x-1">
                  <DiscIcon size={16} />
                  <span>CD ROM</span>
                </div>
              )
            }
          ]}
        />

        <div className="mt-4 max-h-[240px] overflow-y-auto">
          {available.length === 0 && (
            <div className="text-sm text-neutral-400">No catalog images for this mode</div>
          )}
          <Radio.Group
            className="flex flex-col space-y-2"
            value={selected}
            onChange={(event) => setSelected(event.target.value as number)}
          >
            {available.map((image) => (
              <Radio key={image.imageId} value={image.imageId}>
                {image.imageName} ({image.size} B)
              </Radio>
            ))}
          </Radio.Group>
        </div>

        <div className="mt-4 flex items-center justify-between">
          <span>Connected</span>
          <Switch
            checked={isConnected}
            disabled={!writable || isLoading}
            onChange={toggleConnection}
          />
        </div>

        <div className="mt-4 flex justify-end space-x-2">
          <Button disabled={!writable || selected === null || isLoading} onClick={mount}>
            Mount
          </Button>
        </div>

        {!writable && (
          <div className="mt-2 text-sm text-neutral-500">
            Virtual media changes are not enabled in this build.
          </div>
        )}
      </Modal>
    </>
  );
};
