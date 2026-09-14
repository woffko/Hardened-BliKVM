import { useEffect, useState } from 'react';
import { message, Modal, Segmented, Tooltip } from 'antd';
import clsx from 'clsx';
import { useAtomValue, useSetAtom } from 'jotai';
import {
  ArrowBigDownDashIcon,
  ArrowBigUpDashIcon,
  DiscIcon,
  HardDriveIcon,
  LoaderCircleIcon,
  PackageIcon
} from 'lucide-react';
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
const isIso = (name: string) => name.toLowerCase().endsWith('.iso');

/**
 * Hardened virtual-media panel with the original operator UX: an image row is
 * clickable and toggles mount/unmount. A mount detaches first (KVMD requires a
 * detached drive before its image changes), selects the opaque catalog ID and
 * connects after the short media-policy cooldown.
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
  const [busyImage, setBusyImage] = useState<string | null>(null);

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

  async function toggle(image: MediaImage) {
    if (busyImage || !writable) return;
    setBusyImage(image.imageName);
    try {
      if (mounted === image.imageName) {
        if (await run({ action: 'set_connected', connected: false })) {
          setMounted(null);
        }
        return;
      }
      if (!(await run({ action: 'set_connected', connected: false }))) return;
      await delay(COOLDOWN_MS);
      if (
        !(await run({
          action: 'select',
          imageId: image.imageId,
          mode: modeFor(mode),
          writable: false
        }))
      ) {
        return;
      }
      await delay(COOLDOWN_MS);
      if (await run({ action: 'set_connected', connected: true })) {
        setMounted(image.imageName);
      }
    } finally {
      setBusyImage(null);
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
            mounted ? 'text-blue-500' : 'text-neutral-300 hover:text-white'
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
          <span>{t('image.mountMode')}</span>
          <Segmented
            value={mode}
            disabled={mounted !== null || busyImage !== null}
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

        <div className="mt-4 flex max-h-[320px] flex-col overflow-y-auto pb-2">
          {available.length === 0 && (
            <div className="flex items-center justify-center space-x-2 py-5 text-neutral-500">
              <PackageIcon size={18} />
              <span className="text-sm">No catalog images for this mode</span>
            </div>
          )}
          {available.map((image) => {
            const active = mounted === image.imageName;
            const busy = busyImage === image.imageName;
            return (
              <div
                key={image.imageId}
                className={clsx(
                  'group flex select-none items-center space-x-1 rounded px-1 py-2',
                  writable
                    ? 'cursor-pointer hover:bg-neutral-700/70'
                    : 'cursor-not-allowed opacity-60',
                  active && 'text-blue-500'
                )}
                onClick={() => toggle(image)}
              >
                <div className="flex h-[24px] w-[24px] items-center justify-center">
                  {busy ? (
                    <LoaderCircleIcon className="animate-spin" size={18} />
                  ) : (
                    <PackageIcon size={18} />
                  )}
                </div>
                <div className="flex-1 truncate">{image.imageName}</div>
                <div
                  className={clsx(
                    'rounded border px-1.5 py-0.5 text-[10px] font-semibold uppercase leading-none',
                    isIso(image.imageName)
                      ? 'border-blue-500/50 bg-blue-500/10 text-blue-300'
                      : 'border-emerald-500/50 bg-emerald-500/10 text-emerald-300'
                  )}
                >
                  {isIso(image.imageName) ? 'ISO' : 'IMG'}
                </div>
                <div className="flex h-[24px] w-[24px] items-center justify-center rounded">
                  {active ? (
                    <ArrowBigDownDashIcon
                      size={22}
                      className="hidden text-red-500 group-hover:block"
                    />
                  ) : (
                    <ArrowBigUpDashIcon
                      size={22}
                      className="hidden text-blue-500 group-hover:block"
                    />
                  )}
                </div>
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
