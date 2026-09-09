import { useEffect, useRef, useState } from 'react';
import type { ApplicationUploadReceipt } from '@/types/application-update.ts';

import { uploadApplicationArchive } from '@/api/application-update.ts';
import {
  isApplicationUploadReceipt,
  validApplicationArchiveSize
} from '@/lib/application-update.ts';

type UploadState =
  | 'idle'
  | 'invalidFile'
  | 'uploading'
  | 'verifying'
  | 'staged'
  | 'rejected'
  | 'busyOrQuota'
  | 'uncertain'
  | 'cancelled';
type Snapshot = { state: UploadState; percent: number; receipt: ApplicationUploadReceipt | null };
const initial: Snapshot = { state: 'idle', percent: 0, receipt: null };

export function useApplicationUpload(enabled: boolean) {
  const [file, setFile] = useState<File | null>(null);
  const [snapshot, setSnapshot] = useState<Snapshot>(initial);
  const active = useRef<AbortController | null>(null);

  useEffect(
    () => () => {
      active.current?.abort();
      active.current = null;
    },
    []
  );
  useEffect(() => {
    if (!enabled && active.current) {
      active.current.abort();
      active.current = null;
      setSnapshot((value) => ({ ...value, state: 'cancelled', receipt: null }));
    }
  }, [enabled]);

  const select = (candidate: File | null) => {
    if (active.current) return;
    setFile(candidate);
    setSnapshot(
      candidate && !validApplicationArchiveSize(candidate.size)
        ? { ...initial, state: 'invalidFile' }
        : initial
    );
  };
  const cancel = () => {
    if (!active.current) return;
    active.current.abort();
    active.current = null;
    setSnapshot((value) => ({ ...value, state: 'cancelled', receipt: null }));
  };
  const start = async () => {
    if (!enabled || active.current || !file || !validApplicationArchiveSize(file.size)) return;
    const controller = new AbortController();
    active.current = controller;
    setSnapshot({ ...initial, state: 'uploading' });
    try {
      const response = await uploadApplicationArchive(file, controller.signal, (loaded) => {
        if (active.current !== controller) return;
        const percent = Math.max(0, Math.min(100, Math.floor((loaded / file.size) * 100)));
        // 100% is transport progress, never signature/publication acceptance.
        setSnapshot({ state: percent === 100 ? 'verifying' : 'uploading', percent, receipt: null });
      });
      if (active.current !== controller) return;
      if (response.code === 0 && isApplicationUploadReceipt(response.data)) {
        setSnapshot({ state: 'staged', percent: 100, receipt: response.data });
      } else {
        setSnapshot({
          ...initial,
          state:
            response.code === -1 ? 'rejected' : response.code === -409 ? 'busyOrQuota' : 'uncertain'
        });
      }
    } catch {
      if (active.current === controller) setSnapshot({ ...initial, state: 'uncertain' });
    } finally {
      if (active.current === controller) active.current = null;
    }
  };
  return {
    ...snapshot,
    file,
    select,
    cancel,
    start,
    busy: snapshot.state === 'uploading' || snapshot.state === 'verifying'
  };
}
