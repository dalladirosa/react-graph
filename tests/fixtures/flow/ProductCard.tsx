import { formatPrice } from "./format";

export function ProductCard({ product, onAdd }: any) {
  return <button onClick={() => onAdd(product.id)}>{formatPrice(product.price)}</button>;
}
