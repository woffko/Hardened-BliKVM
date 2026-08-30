import { useEffect } from 'react';
import type { CapabilityManifest } from '@/types/capabilities.ts';
import { useSetAtom } from 'jotai';

import { getCapabilities } from '@/api/capabilities.ts';
import {
  capabilitiesAtom,
  capabilitiesErrorAtom,
  capabilitiesLoadingAtom
} from '@/jotai/capabilities.ts';

let pendingRequest: Promise<CapabilityManifest> | null = null;

function loadCapabilities() {
  if (!pendingRequest) {
    pendingRequest = getCapabilities().then((response) => {
      if (response.code !== 0 || response.data.schemaVersion !== 1) {
        throw new Error(response.msg || 'Unsupported capability manifest');
      }
      return response.data;
    });
  }
  return pendingRequest;
}

export function useCapabilityBootstrap() {
  const setCapabilities = useSetAtom(capabilitiesAtom);
  const setLoading = useSetAtom(capabilitiesLoadingAtom);
  const setError = useSetAtom(capabilitiesErrorAtom);

  useEffect(() => {
    let cancelled = false;

    loadCapabilities()
      .then((manifest) => {
        if (!cancelled) {
          setCapabilities(manifest);
          setError(null);
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setCapabilities(null);
          setError(error instanceof Error ? error.message : 'Capability request failed');
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [setCapabilities, setError, setLoading]);
}
