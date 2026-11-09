import { useContext } from "react";
import { ThemeContext } from "./ThemeContext";

export function Old() {
  return <i className={useContext(ThemeContext)} />;
}
