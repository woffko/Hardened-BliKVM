import { useEffect, useState } from 'react';
import { Button, message, Modal, Segmented, Tooltip } from 'antd';
import clsx from 'clsx';
import { useAtomValue, useSetAtom } from 'jotai';
import { DiscIcon, HardDriveIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import {
  getMediaCatalog,
  submitVirtualMedia,
  type MediaImage,
  type VirtualMediaCommand,
  type VirtualMediaMode
} from '@/api/virtual-media.ts';
import { isWritable } from '@/lib/capabilities.ts';
import { newOperationId } from '@/lib/hid-lease.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { submenuOpenCountAtom } from '@/jotai/settings.ts';

import { Tips } from './tips.tsx';

const COOLDOWN_MS = 2200;
const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const modeFor = (value: string): VirtualMediaMode =>
  value === 'cd-rom' ? 'cd_rom' : 'removable_disk';

/**
 * Hardened virtual-media panel with the original single-toggle UX: one button
 * per image mounts or unmounts it. A mount selects the opaque catalog ID and
 * then connects, honouring the media policy's short global mutation cooldown.
 */
export const HardenedImage = () => {
  const { t } = useTranslation();
  const manifest = useAtomValue(capabilitiesAtom);
  const writable = Boolean(manifest && isWritable(manifest.virtualMedia.status));
  const setSubmenuOpenCount = useSetAtom(submenuOpenCountAtom);

  const [isModalOpen, setIsModalOpen] = useState(false);
  const [images, setImages] = useState<MediaImage[]>([]);
  const [mode, setMode] = useState('mass-storage');
  const [mounted, setMounted] = useState<string | null>(null);
  const [isConnected, setIsConnected] = useState(false);
  const [isBusy, setIsBusy] = useState(false);

  useEffect(() => {
    if (!isModalOpen) return;
    getMediaCatalog()
      .then((rsp) => {
        if (rsp.code === 0) {
          setImages(rsp.data.images);
        }
      })
      .catch(() => undefined);
  }, [isModalOpen]);

  function toggleModal(open: boolean) {
    setIsModalOpen(open);
    setSubmenuOpenCount((count) => (open ? count + 1 : Math.max(0, count - 1)));
  }

  async function run(command: VirtualMediaCommand): Promise<boolean> {
    try {
      const rsp = await submitVirtualMedia(newOperationId(), command);
      if (rsp.data.applied) {
        return true;
      }
      message.error(rsp.data.rejection || 'Virtual media command rejected');
      return false;
    } catch {
      message.error('Virtual media command failed');
      return false;
    }
  }

  async function mount(image: MediaImage) {
    if (isBusy) return;
    setIsBusy(true);
    try {
      const selected = await run({
        action: 'select',
        imageId: image.imageId,
        mode: modeFor(mode),
        writable: false
      });
      if (!selected) return;
      await delay(COOLDOWN_MS);
      if (!(await run({ action: 'set_connected', connected: true }))) return;
      setMounted(image.imageName);
      setIsConnected(true);
    } finally {
      setIsBusy(false);
    }
  }

  async function unmount() {
    if (isBusy) return;
    setIsBusy(true);
    try {
      if (await run({ action: 'set_connected', connected: false })) {
        setIsConnected(false);
        setMounted(null);
      }
    } finally {
      setIsBusy(false);
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

        <div className="mt-4 flex items-center justify-between">
          <span>Mount mode</span>
          <Segmented
            value={mode}
            disabled={isConnected || isBusy}
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
        </div>

        <div className="mt-4 max-h-[260px] space-y-2 overflow-y-auto">
          {available.length === 0 && (
            <div className="text-sm text-neutral-400">No catalog images for this mode</div>
          )}
          {available.map((image) => {
            const isMounted = mounted === image.imageName && isConnected;
            return (
              <div
                key={image.imageId}
                className="flex items-center justify-between rounded border border-neutral-700/40 px-3 py-2"
              >
                <span className="min-w-0 truncate">
                  {image.imageName}{' '}
                  <span className="text-xs text-neutral-500">({image.size} B)</span>
                </span>
                <Button
                  size="small"
                  type={isMounted ? 'default' : 'primary'}
                  disabled={!writable || isBusy}
                  onClick={() => (isMounted ? unmount() : mount(image))}
                >
                  {isMounted ? 'Unmount' : 'Mount'}
                </Button>
              </div>
            );
          })}
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
