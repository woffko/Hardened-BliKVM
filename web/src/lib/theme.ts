// Colours of the interface: a dark or light theme (or the system's) and an
// accent. Both are kept per browser, like the other appearance settings.
//
// The theme works through CSS variables (assets/styles/index.css): the light
// theme turns Tailwind's neutral scale around and makes `white` the dark
// contrast colour; an element with the `theme-fixed` class keeps the dark
// values (what lies over the video, the always-light virtual keyboard). The
// accent replaces Tailwind's blue shades and antd's primary colour.

export type ThemeMode = 'dark' | 'light' | 'system';
export const LIGHT_QUERY = '(prefers-color-scheme: light)';
export const THEME_MODES: ThemeMode[] = ['dark', 'light', 'system'];

type Shades = Record<'50' | '200' | '300' | '400' | '500' | '600', string>;

// Tailwind's palette, as "r g b" for `rgb(var(--…) / alpha)`.
export const ACCENTS: Record<string, { shades: Shades; primary?: string }> = {
  // The default: Tailwind's blue and antd's own primary, as before.
  blue: {
    shades: {
      '50': '239 246 255',
      '200': '191 219 254',
      '300': '147 197 253',
      '400': '96 165 250',
      '500': '59 130 246',
      '600': '37 99 235'
    }
  },
  green: {
    shades: {
      '50': '240 253 244',
      '200': '187 247 208',
      '300': '134 239 172',
      '400': '74 222 128',
      '500': '34 197 94',
      '600': '22 163 74'
    },
    primary: '#16a34a'
  },
  cyan: {
    shades: {
      '50': '236 254 255',
      '200': '165 243 252',
      '300': '103 232 249',
      '400': '34 211 238',
      '500': '6 182 212',
      '600': '8 145 178'
    },
    primary: '#0891b2'
  },
  violet: {
    shades: {
      '50': '245 243 255',
      '200': '221 214 254',
      '300': '196 181 253',
      '400': '167 139 250',
      '500': '139 92 246',
      '600': '124 58 237'
    },
    primary: '#7c3aed'
  },
  pink: {
    shades: {
      '50': '253 242 248',
      '200': '251 207 232',
      '300': '249 168 212',
      '400': '244 114 182',
      '500': '236 72 153',
      '600': '219 39 119'
    },
    primary: '#db2777'
  },
  red: {
    shades: {
      '50': '254 242 242',
      '200': '254 202 202',
      '300': '252 165 165',
      '400': '248 113 113',
      '500': '239 68 68',
      '600': '220 38 38'
    },
    primary: '#dc2626'
  },
  orange: {
    shades: {
      '50': '255 247 237',
      '200': '254 215 170',
      '300': '253 186 116',
      '400': '251 146 60',
      '500': '249 115 22',
      '600': '234 88 12'
    },
    primary: '#ea580c'
  },
  amber: {
    shades: {
      '50': '255 251 235',
      '200': '253 230 138',
      '300': '252 211 77',
      '400': '251 191 36',
      '500': '245 158 11',
      '600': '217 119 6'
    },
    primary: '#d97706'
  }
};
export const DEFAULT_ACCENT = 'blue';

export function normalizeThemeMode(value: string | null | undefined): ThemeMode {
  return THEME_MODES.includes(value as ThemeMode) ? (value as ThemeMode) : 'dark';
}

export function normalizeAccent(value: string | null | undefined): string {
  return value && value in ACCENTS ? value : DEFAULT_ACCENT;
}

/** The theme in force: the system's preference stands in for `system`. */
export function resolveTheme(mode: ThemeMode, systemPrefersLight: boolean): 'dark' | 'light' {
  if (mode === 'system') return systemPrefersLight ? 'light' : 'dark';
  return mode;
}

/** Puts the theme and the accent on the document. */
export function applyTheme(theme: 'dark' | 'light', accent: string) {
  const root = document.documentElement;
  root.dataset.theme = theme;
  const shades = ACCENTS[normalizeAccent(accent)].shades;
  for (const [shade, value] of Object.entries(shades)) {
    root.style.setProperty(`--color-accent-${shade}`, value);
  }
}
