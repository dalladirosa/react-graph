import { useProducts } from "./useProducts";
import { addToCart } from "./cart";
import { ProductCard } from "./ProductCard";

export function Shop() {
  const products = useProducts();
  const handleAdd = (id: string) => {
    addToCart(id);
  };
  return (
    <div>
      {products.map((p: any) => (
        <ProductCard key={p.id} product={p} onAdd={handleAdd} />
      ))}
    </div>
  );
}
