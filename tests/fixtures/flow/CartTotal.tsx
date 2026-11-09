import { formatPrice } from "./format";

export function CartTotal({ total }: { total: number }) {
  return <span>{formatPrice(total)}</span>;
}
