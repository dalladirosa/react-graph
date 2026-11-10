import { useEffect, useMemo, useState } from "react";

export function Conditional({ show }: any) {
  const [a] = useState(0);
  if (show) {
    useEffect(() => {}, []);
  }
  const b = show ? useMemo(() => 1, []) : 0;
  const c = show && useState(1);
  for (const x of [1]) {
    useState(x);
  }
  useEffect(() => {
    useState(2);
  }, []);
  return <div>{a}{b}{String(c)}</div>;
}

export function EarlyReturn({ ready }: any) {
  if (!ready) return null;
  const [v] = useState(0);
  return <p>{v}</p>;
}

export function Fine({ ready }: any) {
  const [v, setV] = useState(0);
  useEffect(() => {
    setV(1);
  }, []);
  const label = ready ? "yes" : "no";
  if (!ready) return null;
  return <p>{v}{label}</p>;
}

function notAHook() {
  return useState(0);
}

export function useGood() {
  const [x] = useState(0);
  return x;
}

export function Leaky() {
  useEffect(() => {
    window.addEventListener("resize", onResize);
  }, []);
  useEffect(() => {
    const id = setInterval(tick, 1000);
    return () => clearInterval(id);
  }, []);
  useEffect(() => {
    const unsub = store.subscribe(cb);
  }, []);
  useEffect(() => {
    document.title = "x";
  }, []);
  return null;
}

export function Derived({ items, query }: any) {
  const [filtered, setFiltered] = useState([]);
  const [open, setOpen] = useState(false);
  const [count, setCount] = useState(0);
  useEffect(() => {
    setFiltered(items.filter((i: any) => i.includes(query)));
  }, [items, query]);
  useEffect(() => {
    setOpen(false);
  }, [query]);
  useEffect(() => {
    setCount((c: number) => c + 1);
  }, [items]);
  useEffect(() => {
    fetchStuff(query).then(setCount);
  }, [query]);
  return <i>{filtered.length}{String(open)}{count}</i>;
}

export function List({ rows }: any) {
  return (
    <ul>
      {rows.map((r: any) => <li>{r}</li>)}
      {rows.map((r: any) => <li key={r.id}>{r}</li>)}
      {rows.map((r: any) => {
        return <><b /></>;
      })}
      {rows.map((r: any) => r.name)}
      {rows.map((r: any) => <li {...r} />)}
    </ul>
  );
}

// the draft is also edited by the user: the effect only re-syncs it when the task changes
export function Drawer({ task }: any) {
  const [draft, setDraft] = useState(task.title);
  useEffect(() => {
    setDraft(task.title);
  }, [task.id]);
  return <input value={draft} onChange={(e) => setDraft(e.target.value)} />;
}

// removes itself after the first event: nothing to clean up
export function Once() {
  useEffect(() => {
    window.addEventListener("click", () => {}, { once: true });
  }, []);
  return null;
}
