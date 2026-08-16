import type { Element } from "hast";

export function basePathLinks(basePath: string) {
  const normalizedBase = `/${basePath.replace(/^\/+|\/+$/g, "")}`;

  return {
    name: "slash:base-path-links",
    element: {
      filter: ["a"],
      visit(node: Element) {
        const href = node.properties?.href;
        if (
          typeof href === "string" &&
          href.startsWith("/") &&
          !href.startsWith("//") &&
          href !== normalizedBase &&
          !href.startsWith(`${normalizedBase}/`)
        ) {
          return {
            ...node,
            properties: {
              ...node.properties,
              href: `${normalizedBase}${href}`,
            },
          };
        }
      },
    },
  };
}