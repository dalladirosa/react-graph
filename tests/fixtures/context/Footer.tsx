import { useContext } from "react";
import { ThemeContext } from "./ThemeContext";

export function Footer() {
  const theme = useContext(ThemeContext);
  return <footer className={theme} />;
}
