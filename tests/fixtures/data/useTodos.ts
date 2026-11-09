import { useMutation, useQuery } from "@tanstack/react-query";
import { getProducts } from "./api";

export function useTodos() {
  return useQuery({ queryKey: ["todos"], queryFn: getProducts });
}

export function useAddTodo() {
  return useMutation({ mutationFn: getProducts });
}

export function useDeep() {
  return useTodos();
}
