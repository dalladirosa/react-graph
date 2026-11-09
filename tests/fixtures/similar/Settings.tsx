import { useEffect, useState } from "react";

// structurally unrelated to the lists
export function Settings({ initial, onSave }: { initial: Record<string, boolean>; onSave: (v: Record<string, boolean>) => void }) {
  const [values, setValues] = useState(initial);
  const [dirty, setDirty] = useState(false);
  useEffect(() => {
    if (!dirty) return;
    const t = setTimeout(() => onSave(values), 500);
    return () => clearTimeout(t);
  }, [dirty, values, onSave]);
  return (
    <form onSubmit={(e) => e.preventDefault()}>
      {Object.keys(values).map((k) => (
        <label key={k}>
          <input
            type="checkbox"
            checked={values[k]}
            onChange={() => {
              setValues({ ...values, [k]: !values[k] });
              setDirty(true);
            }}
          />
          {k}
        </label>
      ))}
    </form>
  );
}

// tiny duplicates are below the size cutoff
export function double(n: number) {
  return n * 2;
}
export function triple(n: number) {
  return n * 3;
}
