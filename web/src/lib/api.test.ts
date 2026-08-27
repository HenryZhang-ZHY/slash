import { afterEach, describe, expect, it, vi } from "vitest";

import { accessTokenApi, activityApi, api, testEngineApi } from "./api";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("activityApi", () => {
  it("preserves GitHub identifiers and encodes history filters", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({ items: [], next_cursor: "next-page" }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    await activityApi.listInvocations({
      installationId: "9007199254740993",
      repositoryId: "9007199254740995",
      owner: "octo-org",
      repo: "rocket ship",
      status: "completed",
      command: "deploy",
      cursor: "cursor+/=",
      limit: 25,
    });

    expect(fetchMock).toHaveBeenCalledWith(
      "/api/invocations?installation_id=9007199254740993&repository_id=9007199254740995&owner=octo-org&repo=rocket+ship&status=completed&command=deploy&cursor=cursor%2B%2F%3D&limit=25",
      { credentials: "same-origin", headers: undefined },
    );
  });

  it("loads a page of repositories for an installation", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          items: [{ id: "9007199254740995", name: "slash", full_name: "octo/slash", owner: "octo", private: true }],
          next_cursor: null,
        }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const page = await activityApi.listRepositories("9007199254740993", "page-2", 50);

    expect(page.items[0].id).toBe("9007199254740995");
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/github/installations/9007199254740993/repositories?cursor=page-2&limit=50",
      { credentials: "same-origin", headers: undefined },
    );
  });
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

  it("loads runs and the test executions captured in a selected run", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            total: 1,
            limit: 100,
            offset: 0,
            items: [
              {
                id: "run-id-42",
                run_ref: "run-42",
                ci_provider: "github_actions",
                invocation_id: null,
                started_at: "2026-08-12T02:30:00Z",
                finished_at: null,
                last_captured: "2026-08-12T02:31:00Z",
                execution_count: 2,
                passed_count: 1,
                failed_count: 1,
                skipped_count: 0,
                errored_count: 0,
                total_duration_ms: 100,
              },
            ],
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
      )
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            total: 2,
            limit: 100,
            offset: 0,
            items: [
              {
                id: "execution-1",
                test_id: "test-1",
                test_name: "cart > adds item",
                test_state: "enabled",
                file: "src/cart.test.ts",
                line_no: 7,
                status: "passed",
                duration_ms: 24,
                stack: null,
                captured_at: "2026-08-12T02:31:00Z",
              },
            ],
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
      );
    vi.stubGlobal("fetch", fetchMock);

    const runs = await testEngineApi.listRuns("suite-1", 100, 0);
    const executions = await testEngineApi.listRunExecutions("run-id-42", 100, 0);

    expect(runs.items[0].failed_count).toBe(1);
    expect(executions.items[0].test_name).toBe("cart > adds item");
    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      "/api/test-engine/suites/suite-1/runs?limit=100&offset=0",
      { credentials: "same-origin", headers: undefined },
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      "/api/test-engine/runs/run-id-42/executions?limit=100&offset=0",
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
