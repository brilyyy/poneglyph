// Dark mode via the `dark` class on <html>. A matching inline script in
// index.html applies the stored theme pre-paint (no flash).

const KEY = 'poneglyph-theme'

export type Theme = 'light' | 'dark'

export function getTheme(): Theme {
  if (typeof localStorage !== 'undefined' && localStorage.getItem(KEY) === 'dark') return 'dark'
  return 'light'
}

export function setTheme(theme: Theme) {
  localStorage.setItem(KEY, theme)
  document.documentElement.classList.toggle('dark', theme === 'dark')
}

export function toggleTheme(): Theme {
  const next: Theme = getTheme() === 'dark' ? 'light' : 'dark'
  setTheme(next)
  return next
}
