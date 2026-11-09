import { useCallback, useMemo, useState } from "react";

function debounce<A extends unknown[]>(fn: (...a: A) => void, ms: number) {
  let t: ReturnType<typeof setTimeout>;
  return (...a: A) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...a), ms);
  };
}

function Table({ rows, options, onSort, onSearch, format, render }: any) {
  return <table />;
}

export function List({ items }: { items: string[] }) {
  const [query, setQuery] = useState("");
  // values: passing these as props is not a handler edge
  const rows = useMemo(() => items.filter((i) => i.includes(query)), [items, query]);
  const options = useMemo(() => {
    return { dense: true };
  }, []);
  // functions: these are handler edges
  const sortRows = useCallback(() => {}, []);
  const format = useMemo(() => (s: string) => s.toUpperCase(), []);
  const render = useMemo(() => {
    return function renderRow(s: string) {
      return s;
    };
  }, []);
  // opaque call result passed to an event prop: kept
  const onSearch = useMemo(() => debounce(setQuery, 200), []);

  return <Table rows={rows} options={options} onSort={sortRows} onSearch={onSearch} format={format} render={render} />;
}
