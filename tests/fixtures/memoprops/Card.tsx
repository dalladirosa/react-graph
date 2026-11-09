import { memo } from "react";

export const Card = memo(function Card({ item, onSelect, style, options }: any) {
  return (
    <div style={style} onClick={onSelect} data-n={options?.length}>
      {item.name}
    </div>
  );
});
