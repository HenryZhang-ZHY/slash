export const DEFAULT_LOCALE = "en" as const;
export const LOCALE_STORAGE_KEY = "docs-locale";

export const LOCALES = {
  en: {
    languageTag: "en",
    path: "en",
    label: "English",
    ogLocale: "en_US",
  },
  "zh-Hans": {
    languageTag: "zh-Hans",
    path: "zh-hans",
    label: "简体中文",
    ogLocale: "zh_CN",
  },
} as const;

export type Locale = keyof typeof LOCALES;
export type LocaleDefinition = (typeof LOCALES)[Locale];

export interface LocaleFallback {
  requestedLocale: Locale;
  sourceLocale: Locale;
}

export interface LocalizedStaticPath<T> {
  params: { slug: string };
  props: { entry: T; fallback?: LocaleFallback };
}

const localeList = Object.values(LOCALES);

const translations = {
  en: {
    skipToContent: "Skip to content",
    navigation: {
      sections: "Sections",
      open: "Open navigation",
      site: "Site navigation",
      title: "Navigation",
      close: "Close navigation",
      filter: "Filter…",
      filterLabel: "Filter navigation",
      onThisPage: "On this page",
      tableOfContents: "Table of contents",
      jumpToSection: "Jump to section",
      overview: "Overview",
      toggleSection: (label: string) => `Toggle ${label} section`,
    },
    search: {
      label: "Search documentation",
      shortLabel: "Search",
      placeholder: "Search documentation…",
      initial: "Type to search…",
      navigate: "navigate",
      select: "select",
      close: "close",
      unavailableAfterBuild: "Search is available after a production build.",
      searching: "Searching…",
      noResults: "No results found.",
      unavailable: "Search is temporarily unavailable.",
      untitled: "Untitled",
    },
    theme: { toggle: "Toggle dark mode" },
    pagination: { label: "Pagination", previous: "Previous", next: "Next" },
    pageActions: {
      updated: "Updated",
      copyPage: "Copy page",
      copied: "Copied",
      copyFailed: "Couldn't copy",
      viewMarkdown: "View as Markdown",
    },
    language: { select: "Select language" },
    home: {
      description: "Slash commands for GitHub pull requests, done right.",
      cards: [
        ["getting-started", "Get started", "Install and connect the GitHub App"],
        ["slash-config-reference", "Configure commands", "Define arguments and permissions"],
        ["test-engine", "Test Engine", "Collect results and quarantine flaky tests"],
        ["permissions", "Permissions", "Understand private and public repository policy"],
        ["self-host", "Self Hosting", "Run Slash on your own infrastructure"],
        ["api", "API reference", "Integrate with every server endpoint"],
      ],
    },
    editPage: "Edit this page",
    draft: "Draft",
    forHumans: "For humans",
    translationFallback: "This page is not available in your language yet. Showing the English source.",
  },
  "zh-Hans": {
    skipToContent: "跳到正文",
    navigation: {
      sections: "文档分区",
      open: "打开导航",
      site: "站点导航",
      title: "导航",
      close: "关闭导航",
      filter: "筛选…",
      filterLabel: "筛选导航",
      onThisPage: "本页内容",
      tableOfContents: "本页目录",
      jumpToSection: "跳到章节",
      overview: "概览",
      toggleSection: (label: string) => `展开或折叠“${label}”`,
    },
    search: {
      label: "搜索文档",
      shortLabel: "搜索",
      placeholder: "搜索文档…",
      initial: "输入关键词开始搜索…",
      navigate: "移动",
      select: "选择",
      close: "关闭",
      unavailableAfterBuild: "搜索仅在生产构建后可用。",
      searching: "正在搜索…",
      noResults: "没有找到结果。",
      unavailable: "搜索暂时不可用。",
      untitled: "无标题",
    },
    theme: { toggle: "切换深色模式" },
    pagination: { label: "分页导航", previous: "上一页", next: "下一页" },
    pageActions: {
      updated: "更新于",
      copyPage: "复制页面",
      copied: "已复制",
      copyFailed: "复制失败",
      viewMarkdown: "查看 Markdown",
    },
    language: { select: "选择语言" },
    home: {
      description: "正确实现 GitHub Pull Request 的 Slash Commands。",
      cards: [
        ["getting-started", "快速开始", "安装并连接 GitHub App"],
        ["slash-config-reference", "配置命令", "定义参数与权限"],
        ["test-engine", "Test Engine", "收集测试结果并隔离 flaky 测试"],
        ["permissions", "命令权限", "了解私有与公共仓库的权限策略"],
        ["self-host", "自托管（Self Hosting）", "在你自己的基础设施上运行 Slash"],
        ["api", "API 参考", "接入全部服务端接口"],
      ],
    },
    editPage: "编辑此页",
    draft: "草稿",
    forHumans: "面向读者",
    translationFallback: "此页面尚无简体中文翻译，当前显示英文原文。",
  },
} as const;

export function getTranslations(locale: Locale) {
  return translations[locale];
}

export function getLocaleByEntryId(entryId?: string): LocaleDefinition {
  const segment = entryId?.split("/")[0]?.toLowerCase();
  return localeList.find((locale) => locale.path === segment) ?? LOCALES[DEFAULT_LOCALE];
}

function normalizeBasePath(basePath: string): string {
  if (!basePath || basePath === "/") return "";
  return `/${basePath.replace(/^\/+|\/+$/g, "")}`;
}

export function getLocaleByPath(pathname: string, basePath = ""): LocaleDefinition {
  const base = normalizeBasePath(basePath);
  const path = base && (pathname === base || pathname.startsWith(`${base}/`))
    ? pathname.slice(base.length)
    : pathname;
  const segment = path.split(/[?#]/, 1)[0].split("/").filter(Boolean)[0]?.toLowerCase();
  return localeList.find((locale) => locale.path === segment) ?? LOCALES[DEFAULT_LOCALE];
}

export function hasLocalePath(pathname: string, basePath = ""): boolean {
  const base = normalizeBasePath(basePath);
  const path = base && (pathname === base || pathname.startsWith(`${base}/`))
    ? pathname.slice(base.length)
    : pathname;
  const segment = path.split(/[?#]/, 1)[0].split("/").filter(Boolean)[0]?.toLowerCase();
  return localeList.some((locale) => locale.path === segment);
}

export function getLocalePath(value: string, target: Locale, basePath = ""): string {
  const suffixIndex = value.search(/[?#]/);
  const pathname = suffixIndex === -1 ? value : value.slice(0, suffixIndex);
  const suffix = suffixIndex === -1 ? "" : value.slice(suffixIndex);
  const base = normalizeBasePath(basePath);
  const path = base && (pathname === base || pathname.startsWith(`${base}/`))
    ? pathname.slice(base.length)
    : pathname;
  const segments = path.split("/").filter(Boolean);

  if (localeList.some((locale) => locale.path === segments[0]?.toLowerCase())) {
    segments.shift();
  }

  const trailingSlash = pathname.endsWith("/") || segments.length === 0;
  const localized = `/${LOCALES[target].path}${segments.length ? `/${segments.join("/")}` : ""}${trailingSlash ? "/" : ""}`;
  return `${base}${localized}${suffix}`;
}

export function withLocaleFallbacks<T>(
  paths: LocalizedStaticPath<T>[],
): LocalizedStaticPath<T>[] {
  const knownSlugs = new Set(paths.map((path) => path.params.slug));
  const defaultPath = LOCALES[DEFAULT_LOCALE].path;
  const defaultPrefix = `${defaultPath}/`;
  const fallbacks: LocalizedStaticPath<T>[] = [];

  for (const path of paths) {
    if (!path.params.slug.startsWith(defaultPrefix)) continue;
    const relativeSlug = path.params.slug.slice(defaultPrefix.length);

    for (const locale of localeList) {
      if (locale.languageTag === DEFAULT_LOCALE) continue;
      const slug = `${locale.path}/${relativeSlug}`;
      if (knownSlugs.has(slug)) continue;
      fallbacks.push({
        params: { slug },
        props: {
          entry: path.props.entry,
          fallback: {
            requestedLocale: locale.languageTag,
            sourceLocale: DEFAULT_LOCALE,
          },
        },
      });
    }
  }

  return [...paths, ...fallbacks];
}
