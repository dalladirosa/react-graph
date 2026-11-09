import { ThemeContext } from "./ThemeContext";
import { Header } from "./Header";
import { Footer } from "./Footer";
import { Plain } from "./Plain";

export function App() {
  return (
    <ThemeContext.Provider value="dark">
      <Header />
      <Footer />
      <Plain />
    </ThemeContext.Provider>
  );
}
