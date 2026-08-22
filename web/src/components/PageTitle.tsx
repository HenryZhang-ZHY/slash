import { useEffect } from 'react'

export function PageTitle({ title }: { title?: string }) {
  useEffect(() => {
    document.title = title ? `${title} · Slash` : 'Slash'
  }, [title])
  return null
}
