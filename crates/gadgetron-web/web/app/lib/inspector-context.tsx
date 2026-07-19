"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

export interface InspectorView {
  id: string;
  title: string;
  content: ReactNode;
  autoOpen?: boolean;
}

interface InspectorContextValue {
  view: InspectorView | null;
  register: (view: InspectorView) => () => void;
}

const InspectorContext = createContext<InspectorContextValue>({
  view: null,
  register: () => () => {},
});

export function InspectorProvider({ children }: { children: ReactNode }) {
  const [entry, setEntry] = useState<{ token: symbol; view: InspectorView } | null>(null);

  const register = useCallback((view: InspectorView) => {
    const token = Symbol(view.id);
    setEntry({ token, view });
    return () => {
      setEntry((current) => current?.token === token ? null : current);
    };
  }, []);

  const value = useMemo<InspectorContextValue>(() => ({
    view: entry?.view ?? null,
    register,
  }), [entry, register]);

  return <InspectorContext.Provider value={value}>{children}</InspectorContext.Provider>;
}

export function useInspector(): InspectorContextValue {
  return useContext(InspectorContext);
}

export function useRegisterInspectorView(view: InspectorView | null) {
  const { register } = useInspector();
  useEffect(() => {
    if (!view) return;
    return register(view);
  }, [register, view]);
}
