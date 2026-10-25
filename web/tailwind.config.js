/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{js,ts,jsx,tsx}'],
  theme: {
    extend: {
      // Theme colours come from CSS variables (src/lib/theme.ts,
      // src/assets/styles/index.css): the light theme turns the neutral scale
      // around and makes `white` its dark contrast colour, and the accent
      // replaces these blue shades.
      colors: {
        white: 'rgb(var(--color-white) / <alpha-value>)',
        neutral: {
          50: 'rgb(var(--color-neutral-50) / <alpha-value>)',
          100: 'rgb(var(--color-neutral-100) / <alpha-value>)',
          200: 'rgb(var(--color-neutral-200) / <alpha-value>)',
          300: 'rgb(var(--color-neutral-300) / <alpha-value>)',
          400: 'rgb(var(--color-neutral-400) / <alpha-value>)',
          500: 'rgb(var(--color-neutral-500) / <alpha-value>)',
          600: 'rgb(var(--color-neutral-600) / <alpha-value>)',
          700: 'rgb(var(--color-neutral-700) / <alpha-value>)',
          800: 'rgb(var(--color-neutral-800) / <alpha-value>)',
          900: 'rgb(var(--color-neutral-900) / <alpha-value>)',
          950: 'rgb(var(--color-neutral-950) / <alpha-value>)'
        },
        // The accent as such: `bg-accent/15`, `text-accent-text`.
        accent: {
          DEFAULT: 'rgb(var(--color-accent-500) / <alpha-value>)',
          text: 'rgb(var(--color-accent-text) / <alpha-value>)'
        },
        blue: {
          50: 'rgb(var(--color-accent-50) / <alpha-value>)',
          200: 'rgb(var(--color-accent-200) / <alpha-value>)',
          300: 'rgb(var(--color-accent-300) / <alpha-value>)',
          400: 'rgb(var(--color-accent-400) / <alpha-value>)',
          500: 'rgb(var(--color-accent-500) / <alpha-value>)',
          600: 'rgb(var(--color-accent-600) / <alpha-value>)'
        }
      }
    }
  },
  plugins: [],
  corePlugins: {
    preflight: false
  }
};
