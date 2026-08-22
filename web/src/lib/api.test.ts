import { afterEach, describe, expect, it, vi } from "vitest";

import { accessTokenApi, api, testEngineApi } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("testEngineApi", () => {
  it("creates a suite with the API wire field names", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          suite: {
            id: "suite-1",
            suite_key: "web",
            owner: "HenryZhang-ZHY",
            repo: "slash",
            total_tests: 0,
            muted: 0,
            skipped: 0,
          },
        }),
        { status: 201, headers: { "Content-Type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const result = await testEngineApi.createSuite(
      "HenryZhang-ZHY",
      "slash",
      "web",
    );

    expect(result.suite.id).toBe("suite-1");
    expect(fetchMock).toHaveBeenCalledWith("/api/test-engine/suites", {
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      method: "POST",
      body: JSON.stringify({
        owner: "HenryZhang-ZHY",
        repo: "slash",
        suite_key: "web",
      }),
    });
  });

  it("creates a named non-expiring collection token", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          id: "token-1",
          name: "Buildkite production",
          token: "collector-token",
          expires_at: null,
        }),
        { status: 201, headers: { "Content-Type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const result = await testEngineApi.issueToken(
      "suite-1",
      "Buildkite production",
      null,
    );

    expect(result.token).toBe("collector-token");
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/test-engine/suites/suite-1/tokens",
      {
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        method: "POST",
        body: JSON.stringify({
          name: "Buildkite production",
          expires_at: null,
        }),
      },
    );
  });

  it("surfaces the API error message and status", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({ message: "suite is owned by another account" }),
          {
            status: 409,
            statusText: "Conflict",
            headers: { "Content-Type": "application/json" },
          },
        ),
      ),
    );

    const request = testEngineApi.createSuite("HenryZhang-ZHY", "slash", "web");

    await expect(request).rejects.toMatchObject({
      message: "suite is owned by another account",
      status: 409,
    });
  });

  it("loads execution history for an individual test case", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          total: 1,
          limit: 100,
          offset: 0,
          items: [
            {
              id: "execution-1",
              status: "passed",
              duration_ms: 24,
              stack: null,
              captured_at: "2026-08-12T02:30:19Z",
              run_id: "run-id-42",
              run_ref: "run-42",
              ci_provider: "vitest",
              started_at: "2026-08-12T02:30:00Z",
              finished_at: "2026-08-12T02:31:00Z",
              invocation_id: null,
            },
          ],
        }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const executions = await testEngineApi.listExecutions("test-1", 100, 0);

    expect(executions.total).toBe(1);
    expect(executions.items).toHaveLength(1);
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/test-engine/tests/test-1/executions?limit=100&offset=0",
      { credentials: "same-origin", headers: undefined },
    );
  });

  it("updates a test case disposition", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ state: "muted" }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const result = await testEngineApi.setTestState("test-1", "muted");

    expect(result.state).toBe("muted");
    expect(fetchMock).toHaveBeenCalledWith("/api/test-engine/tests/test-1/state", {
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      method: "PATCH",
      body: JSON.stringify({ state: "muted" }),
    });
  });
});

describe("authentication API", () => {
  it("loads the authoritative server release version", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ version: "0.8.1" }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(api.meta()).resolves.toEqual({ version: "0.8.1" });
    expect(fetchMock).toHaveBeenCalledWith("/api/meta", {
      credentials: "same-origin",
      headers: undefined,
    });
  });

  it("updates an authenticated user's password credential", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchMock);

    await api.updatePassword({
      email: "oidc@example.com",
      currentPassword: null,
      newPassword: "new-password-1",
    });

    expect(fetchMock).toHaveBeenCalledWith("/api/auth/password", {
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      method: "PUT",
      body: JSON.stringify({
        email: "oidc@example.com",
        currentPassword: null,
        newPassword: "new-password-1",
      }),
    });
  });

  it("loads the authoritative GitHub connection state", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({
            user: {
              id: "user-1",
              email: "user@example.com",
              displayName: "User",
            },
            teams: [],
            connections: {
              github: {
                login: "octocat",
              },
            },
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
      ),
    );

    const me = await api.me();

    expect(me.connections.github).toEqual({
      login: "octocat",
    });
  });

  it("surfaces the backend error field used by auth endpoints", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(JSON.stringify({ error: "not signed in" }), {
          status: 401,
          statusText: "Unauthorized",
          headers: { "Content-Type": "application/json" },
        }),
      ),
    );

    await expect(api.me()).rejects.toMatchObject({
      message: "not signed in",
      status: 401,
    });
  });
});

describe("accessTokenApi", () => {
  it("creates a named token with the selected expiry", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          accessToken: {
            id: "token-1",
            name: "Claude agent",
            createdAt: "2026-08-18T13:00:00Z",
            lastUsedAt: null,
            expiresAt: "2026-11-16T13:00:00Z",
          },
          token: "slash_pat_secret",
        }),
        { status: 201, headers: { "Content-Type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const result = await accessTokenApi.create("Claude agent", 90);

    expect(result.token).toBe("slash_pat_secret");
    expect(fetchMock).toHaveBeenCalledWith("/api/access-tokens", {
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      method: "POST",
      body: JSON.stringify({ name: "Claude agent", expiresInDays: 90 }),
    });
  });

  it("revokes a token by id", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchMock);

    await accessTokenApi.revoke("token-1");

    expect(fetchMock).toHaveBeenCalledWith("/api/access-tokens/token-1", {
      credentials: "same-origin",
      headers: undefined,
      method: "DELETE",
    });
  });
});
