// The locale tags Node's ICU has data for (VE-3728), for the generators of the locale tables in
// src/output/: every language, language-script and language(-script)-region tag that
// `Intl.DateTimeFormat` resolves to exactly that tag, plus `und`. Sorted.
const LETTERS = 'abcdefghijklmnopqrstuvwxyz';
const SCRIPTS = ['Adlm', 'Arab', 'Beng', 'Cyrl', 'Deva', 'Guru', 'Hans', 'Hant', 'Latn', 'Mtei', 'Olck', 'Rohg', 'Vaii'];

export function available(tag) {
  try {
    return new Intl.DateTimeFormat(tag).resolvedOptions().locale === tag;
  } catch {
    return false;
  }
}

export function nodeLocaleTags() {
  const languages = [];
  for (const a of LETTERS) {
    for (const b of LETTERS) {
      languages.push(a + b);
      for (const c of LETTERS) languages.push(a + b + c);
    }
  }
  const regions = [];
  for (const a of LETTERS) for (const b of LETTERS) regions.push((a + b).toUpperCase());
  regions.push('001', '150', '419');

  const tags = new Set(['und']);
  for (const language of languages.filter(available)) {
    tags.add(language);
    const bases = [language, ...SCRIPTS.map((script) => `${language}-${script}`).filter(available)];
    for (const base of bases) {
      tags.add(base);
      for (const region of regions) if (available(`${base}-${region}`)) tags.add(`${base}-${region}`);
    }
  }
  return [...tags].sort();
}

/** A string as a Rust literal, with non-ASCII characters escaped. */
export const rustString = (s) =>
  JSON.stringify(s).replace(/[\u0080-￿]/g, (c) => `\\u{${c.charCodeAt(0).toString(16)}}`);
