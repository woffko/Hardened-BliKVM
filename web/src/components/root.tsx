import { Outlet } from 'react-router-dom';

import { useCapabilityBootstrap } from '@/hooks/useCapabilities.ts';

export const Root = () => {
  useCapabilityBootstrap();

  return (
    <div className="h-screen w-screen">
      <Outlet />
    </div>
  );
};
