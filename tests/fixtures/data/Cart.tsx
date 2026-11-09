import { useDispatch, useSelector } from "react-redux";
import { getProducts, loadUser } from "./api";
import { useDeep } from "./useTodos";
import { useCartStore } from "./store";

export function Cart() {
  const items = useSelector((s: any) => s.items);
  const dispatch = useDispatch();
  const total = useCartStore((s: any) => s.total);
  const todos = useDeep();
  async function refresh() {
    await getProducts();
    await loadUser("1");
  }
  return <button onClick={refresh}>{items.length}{total}{String(todos)}{String(dispatch)}</button>;
}
