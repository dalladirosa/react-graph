import { useCallback, useEffect, useState } from "react";
import { ThemeContext } from "./ThemeContext";
import { ProductCard } from "./ProductCard";
import { CartSummary } from "./CartSummary";
import { SearchBox } from "./SearchBox";
import { FastBadge } from "./Badge";

export function App() {
  const [query, setQuery] = useState("");
  const [products] = useState([{ id: 1, name: "Pen", price: 2 }]);
  const options = { sort: "price" };
  const visible = products.filter((p) => p.name.includes(query));

  // fine: setters are stable
  const handleSearch = useCallback((q: string) => {
    setQuery(q);
  }, []);

  // `query` is missing, and `options` is a new object every render
  useEffect(() => {
    console.log(options.sort, query);
  }, [options]);

  return (
    <ThemeContext.Provider value="light">
      <SearchBox onSearch={handleSearch} />
      {visible.map((p) => (
        <ProductCard key={p.id} product={p} />
      ))}
      <CartSummary />
      <FastBadge count={visible.length} />
    </ThemeContext.Provider>
  );
}
