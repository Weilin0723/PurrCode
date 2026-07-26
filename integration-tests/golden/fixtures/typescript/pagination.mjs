export function page(items, offset, limit) {
  return items.slice(offset, limit);
}
