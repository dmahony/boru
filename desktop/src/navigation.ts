export type Route =
  | { page: "conversations"; conversationId?: string }
  | { page: "directory" | "friends" | "shared-files" | "tunnels" | "settings" | "call" };

const pages = new Set(["directory", "friends", "shared-files", "tunnels", "settings", "call"]);

export function parseRoute(hash: string): Route {
  const parts = hash.replace(/^#\/?/, "").split("/").filter(Boolean).map(part => decodeURIComponent(part));
  if (parts[0] === "conversation" && parts[1]) return { page: "conversations", conversationId: parts[1] };
  if (parts[0] === "conversations" || !parts[0]) return { page: "conversations" };
  if (pages.has(parts[0])) return { page: parts[0] as Exclude<Route, { page: "conversations" }>['page'] };
  return { page: "conversations" };
}

export function routeUrl(route: Route): string {
  return route.page === "conversations"
    ? route.conversationId ? `#/conversation/${encodeURIComponent(route.conversationId)}` : "#/conversations"
    : `#/${route.page}`;
}