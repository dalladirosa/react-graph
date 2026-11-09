import { useTheme } from "./useTheme";

// reads the context through a custom hook
export function Header() {
  const theme = useTheme();
  return <header className={theme} />;
}
