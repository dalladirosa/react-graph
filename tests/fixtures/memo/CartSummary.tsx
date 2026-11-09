import { useEffect, useMemo } from "react";
import { useCart } from "./useCart";

export function CartSummary() {
  const { items, clear } = useCart();
  const total = useMemo(() => items.reduce((s, i) => s + i.price, 0), [items]);
  // no dependency array: recomputed every render
  const label = useMemo(() => `Total: ${total}`);

  // `label` is missing: the title goes stale
  useEffect(() => {
    document.title = label;
  }, []);

  // fine: `clear` is memoized inside useCart
  useEffect(() => {
    if (items.length > 99) clear();
  }, [items, clear]);

  return <p>{label}</p>;
}
