import { memo, useCallback, useContext } from "react";
import { useCart } from "./useCart";
import { ThemeContext } from "./ThemeContext";

export const ProductCard = memo(function ProductCard({ product }: { product: any }) {
  const theme = useContext(ThemeContext);
  const { addItem } = useCart();
  // addItem is recreated on every render inside useCart, so this memo never holds
  const handleAdd = useCallback(() => addItem(product), [addItem, product]);
  return (
    <button className={theme} onClick={handleAdd}>
      {product.name}
    </button>
  );
});
