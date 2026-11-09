import { forwardRef, useEffect, useRef } from "react";

export const SearchBox = forwardRef(function SearchBox(
  { onSearch }: { onSearch: (q: string) => void },
  ref,
) {
  const timer = useRef<number>();
  // no dependency array: runs after every render
  useEffect(() => {
    timer.current = window.setTimeout(() => onSearch(""), 300);
  });
  return <input ref={ref} onChange={(e) => onSearch(e.target.value)} />;
});
