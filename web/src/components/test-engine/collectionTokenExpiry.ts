export type CollectionTokenExpiry = 'none' | '30' | '90' | '365' | 'custom'

const DAY_MS = 24 * 60 * 60 * 1000

export function resolveCollectionTokenExpiry(
  expiry: CollectionTokenExpiry,
  customExpiry: string,
  now = new Date(),
): string | null {
  if (expiry === 'none') return null
  if (expiry === 'custom') return new Date(customExpiry).toISOString()
  return new Date(now.getTime() + Number(expiry) * DAY_MS).toISOString()
}
