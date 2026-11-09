import { memo } from "react";

function Badge({ count }: { count: number }) {
  return <span>{count}</span>;
}

export const FastBadge = memo(Badge);
