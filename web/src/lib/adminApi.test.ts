import { afterEach, describe, expect, it, vi } from 'vitest'

import { adminApi } from './adminApi'

afterEach(() => vi.unstubAllGlobals())

describe('adminApi', () => {
  it('sends the admin secret only in the login request body', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ authenticated: true }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    vi.stubGlobal('fetch', fetchMock)

    await adminApi.login('top-secret')

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/admin/auth/login',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ secret: 'top-secret' }),
        credentials: 'same-origin',
      }),
    )
    expect(fetchMock.mock.calls[0][0]).not.toContain('top-secret')
  })
})
