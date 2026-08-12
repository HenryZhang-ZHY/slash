import { afterEach, describe, expect, it, vi } from "vitest";

import { testEngineApi } from "./api";

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
          token: "collector-token",
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

    expect(result.token).toBe("collector-token");
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
        JSON.stringify([
          {
            id: "execution-1",
            status: "passed",
            duration_ms: 24,
            captured_at: "2026-08-12T02:30:19Z",
            run_ref: "run-42",
            ci_provider: "vitest",
          },
        ]),
        { status: 200, headers: { "Content-Type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const executions = await testEngineApi.listExecutions("test-1");

    expect(executions).toHaveLength(1);
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/test-engine/tests/test-1/executions",
      { credentials: "same-origin", headers: undefined },
    );
  });
});
