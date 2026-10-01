import React, { Suspense } from 'react';
import { Spin } from 'antd';
import ReactDOM from 'react-dom/client';
import { ErrorBoundary } from 'react-error-boundary';
import { HelmetProvider } from 'react-helmet-async';

import { MainError } from './components/main-error.tsx';
import { ThemedApp } from './components/themed-app.tsx';
import { getAccent, getThemeMode } from './lib/localstorage.ts';
import {
  applyTheme,
  LIGHT_QUERY,
  normalizeAccent,
  normalizeThemeMode,
  resolveTheme
} from './lib/theme.ts';

import './i18n';
import './assets/styles/index.css';

const renderApp = () => {
  // Before the first paint, so the page does not flash the other theme.
  applyTheme(
    resolveTheme(normalizeThemeMode(getThemeMode()), window.matchMedia(LIGHT_QUERY).matches),
    normalizeAccent(getAccent())
  );

  return ReactDOM.createRoot(document.getElementById('root')!).render(
    <React.StrictMode>
      <Suspense
        fallback={
          <div className="flex h-screen w-screen items-center justify-center">
            <Spin size="large" />
          </div>
        }
      >
        <ErrorBoundary FallbackComponent={MainError}>
          <HelmetProvider>
            <ThemedApp />
          </HelmetProvider>
        </ErrorBoundary>
      </Suspense>
    </React.StrictMode>
  );
};

async function main() {
  if (import.meta.env.MODE === 'mocked') {
    const { worker } = await import('./mocks/browser');
    await worker.start();
  }
  renderApp();
}

await main();
