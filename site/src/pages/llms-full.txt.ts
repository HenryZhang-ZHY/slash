// Full-corpus markdown for AI agents — every published page in one document.
import { getIndexedEntries, renderEntryAsMarkdown } from "@cloudflare/nimbus-docs";
import { config } from "virtual:nimbus/config";
import { siteUrl } from "@/lib/urls";

export const prerender = true;

export async function GET() {
  const entries = (await getIndexedEntries()).sort((left, right) =>
    left.url.localeCompare(right.url),
  );
  const lines = [`# ${config.title}`, ""];

  if (config.description) {
    lines.push(`> ${config.description}`, "");
  }

  lines.push(`Index: ${siteUrl("/llms.txt")}`, "");

  for (const item of entries) {
    lines.push(`# ${item.title}`, "");
    if (item.description) {
      lines.push(`> ${item.description}`, "");
    }
    lines.push(
      `Source: ${siteUrl(item.url)} · Markdown: ${siteUrl(item.markdownUrl)}`,
      "",
      renderEntryAsMarkdown(item.entry),
      "",
    );
  }

  return new Response(lines.join("\n"), {
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  });
}
