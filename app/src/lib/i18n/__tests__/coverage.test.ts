import { describe, expect, it } from 'vitest';

import enAggregate from '../en';

const LOCALES = [
  'zh-CN',
  'hi',
  'es',
  'ar',
  'fr',
  'bn',
  'pt',
  'de',
  'ru',
  'id',
  'it',
  'ko',
  'pl',
] as const;

interface LocaleModule {
  default: Record<string, string>;
}

/**
 * Eagerly imported locale modules — Vite turns the glob into a static map at
 * build time, so this works in both Vitest and production builds (no dynamic
 * import() at runtime, which CLAUDE.md forbids in app/src code).
 */
const localeModules = import.meta.glob<LocaleModule>('../*.ts', { eager: true });

function loadLocale(locale: string): Record<string, string> {
  const mod = localeModules[`../${locale}.ts`];
  if (!mod) throw new Error(`missing locale file: ${locale}.ts`);
  return mod.default;
}

const enFlat = enAggregate as Record<string, string>;

describe('i18n coverage', () => {
  it.each(LOCALES)('locale %s has a translation file', locale => {
    expect(localeModules[`../${locale}.ts`]).toBeDefined();
  });

  it.each(LOCALES)('locale %s defines every English key', locale => {
    const flat = loadLocale(locale);
    const missing = Object.keys(enFlat).filter(k => !(k in flat));
    expect(missing).toEqual([]);
  });

  it.each(LOCALES)('locale %s defines no keys absent from English', locale => {
    const flat = loadLocale(locale);
    const extra = Object.keys(flat).filter(k => !(k in enFlat));
    expect(extra).toEqual([]);
  });

  it.each(['en', ...LOCALES])('locale %s contains no em dashes', locale => {
    const flat = locale === 'en' ? enFlat : loadLocale(locale);
    const keysWithEmDashes = Object.entries(flat)
      .filter(([, value]) => value.includes('\u2014'))
      .map(([key]) => key);
    expect(keysWithEmDashes).toEqual([]);
  });

  // The OpenHuman Managed search option must name the provider behind it, so
  // the managed path does not read as an unattributed black box (#5136). The
  // provider name is a proper noun, so it stays literal in every locale.
  it.each(['en', ...LOCALES])('locale %s names Exa in the managed search copy', locale => {
    const flat = locale === 'en' ? enFlat : loadLocale(locale);
    expect(flat['settings.search.engineManagedDesc']).toContain('Exa');
  });

  // The Tools settings page shipped with 'Tools desc' as its English value (and
  // a literal translation of that placeholder in several locales) — a note to
  // the author that reached users as body copy.
  it('the Tools settings description is real copy, not a placeholder', () => {
    const value = enFlat['pages.settings.features.toolsDesc'];
    expect(value).toBeDefined();
    expect(value.toLowerCase()).not.toBe('tools desc');
    // Long enough to be a sentence rather than a stub of the title.
    expect(value.length).toBeGreaterThan(20);
  });

  // The referral "Apply" button read "Applying…" at rest: the idle key carried
  // the loading string in all 14 locales, so the two were indistinguishable.
  it.each(['en', ...LOCALES])('locale %s labels the referral apply button at rest', locale => {
    const flat = locale === 'en' ? enFlat : loadLocale(locale);
    const idle = flat['rewards.referralSection.apply'];
    const loading = flat['rewards.referralSection.applying'];
    expect(idle).toBeDefined();
    expect(loading).toBeDefined();
    expect(idle).not.toBe(loading);
    // The loading label carries the ellipsis; the idle one must not.
    expect(idle).not.toContain('…');
    expect(idle).not.toContain('...');
  });

  // Icon-only thread-row actions are named after the thread they act on, so
  // every locale has to keep the interpolation placeholder intact.
  it.each(['en', ...LOCALES])(
    'locale %s keeps the {title} placeholder in thread actions',
    locale => {
      const flat = locale === 'en' ? enFlat : loadLocale(locale);
      expect(flat['chat.renameThreadAria']).toContain('{title}');
      expect(flat['chat.deleteThreadAria']).toContain('{title}');
    }
  );
});
