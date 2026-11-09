export function addToCart(id: string) {
  save(id);
}

function save(id: string) {
  localStorage.setItem("cart", id);
}
