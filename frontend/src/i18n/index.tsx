import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import { detectLocale, translate, type Locale } from "./messages";

interface I18nValue {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  /** Translate a key, with `{name}` interpolation. */
  t: (key: string, vars?: Record<string, string | number>) => string;
}

const I18nContext = createContext<I18nValue | null>(null);

export function I18nProvider({ initial, children }: { initial?: string; children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(() => detectLocale(initial));

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next);
    try {
      localStorage.setItem("local-router.locale", next);
    } catch {
      // Private mode: the choice just will not persist.
    }
    document.documentElement.lang = next;
  }, []);

  const t = useCallback(
    (key: string, vars?: Record<string, string | number>) => translate(locale, key, vars),
    [locale],
  );

  const value = useMemo(() => ({ locale, setLocale, t }), [locale, setLocale, t]);
  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

/** The translator. Every page and component reads its strings through this. */
export function useT() {
  const value = useContext(I18nContext);
  if (!value) throw new Error("useT must be used inside <I18nProvider>");
  return value;
}
