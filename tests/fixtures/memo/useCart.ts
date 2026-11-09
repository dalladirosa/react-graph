import { useCallback, useState } from "react";

export function useCart() {
  const [items, setItems] = useState<any[]>([]);
  const addItem = (item: any) => setItems((xs) => [...xs, item]);
  const clear = useCallback(() => setItems([]), []);
  return { items, addItem, clear };
}
