const rawBasePath = import.meta.env.BASE_URL ?? "/";

export const basePath =
  rawBasePath === "/" ? "" : `/${rawBasePath.replace(/^\/+|\/+$/g, "")}`;

export function withBasePath(value: string): string {
  if (!value.startsWith("/") || value.startsWith("//") || !basePath) {
    return value;
  }

  if (value === basePath || value.startsWith(`${basePath}/`)) {
    return value;
  }

  return value === "/" ? `${basePath}/` : `${basePath}${value}`;
}

export function withoutBasePath(pathname: string): string {
  if (!basePath || (pathname !== basePath && !pathname.startsWith(`${basePath}/`))) {
    return pathname;
  }

  const unbasedPath = pathname.slice(basePath.length);
  return unbasedPath || "/";
}

export function siteUrl(value: string): string {
  return new URL(withBasePath(value), import.meta.env.SITE).href;
}