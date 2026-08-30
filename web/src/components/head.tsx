import { useAtomValue } from 'jotai';
import { Helmet, HelmetData } from 'react-helmet-async';

import { HARDENED_NAME } from '@/lib/hardened.ts';
import { webTitleAtom } from '@/jotai/settings.ts';

type HeadProps = {
  title?: string;
  description?: string;
};

const helmetData = new HelmetData({});

export const Head = ({ title = '', description = '' }: HeadProps = {}) => {
  const webTitle = useAtomValue(webTitleAtom);

  return (
    <Helmet
      helmetData={helmetData}
      title={webTitle ? webTitle : title ? `${title} - ${HARDENED_NAME}` : HARDENED_NAME}
      defaultTitle={webTitle || HARDENED_NAME}
    >
      <meta name="description" content={description} />
    </Helmet>
  );
};
