export function readRunUsage(value: unknown) {
  return typeof value === "object" ? value : null;
}

export function formatTokens(n: number) {
  return `${n} tokens`;
}
