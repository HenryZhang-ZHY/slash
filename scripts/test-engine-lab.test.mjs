import assert from "node:assert/strict";
import test from "node:test";

import { HttpClient, LabError, runExperiment } from "./test-engine-lab.mjs";

class RecordingClient {
  constructor(quarantine) {
    this.quarantine = quarantine;
    this.posts = [];
    this.gets = [];
  }

  async post(path, body, contentType) {
    this.posts.push({ path, body, contentType });
  }

  async getJson(path) {
    this.gets.push(path);
    return this.quarantine;
  }
}

test("exercises normalized replay and collector dialects", async () => {
  const client = new RecordingClient([
    { name: "slash-lab::trial::flaky", state: "muted" },
  ]);

  const result = await runExperiment(client, "trial", { waitSeconds: 0 });

  const normalized = client.posts.filter(({ path }) =>
    path.endsWith("/upload"),
  );
  assert.equal(normalized.length, 4);
  const payloads = normalized.map(({ body }) => JSON.parse(body));
  assert.deepEqual(
    payloads.slice(0, 3).map((payload) => payload.executions[0].status),
    ["failed", "passed", "passed"],
  );
  assert.deepEqual(payloads[0], payloads[3]);
  assert.equal(payloads[0].run_ref, "slash-lab/trial/normalized-1");
  assert.equal(client.posts[4].path, "/v1/test-engine/upload/cargo");
  assert.equal(client.posts[5].path, "/v1/test-engine/upload/vitest");
  assert.deepEqual(client.gets, ["/v1/test-engine/quarantined"]);
  assert.equal(result, "muted");
});

test("rejects an explicitly skipped lab test", async () => {
  const client = new RecordingClient([
    { name: "slash-lab::trial::flaky", state: "skipped" },
  ]);

  await assert.rejects(
    runExperiment(client, "trial", { waitSeconds: 0 }),
    (error) =>
      error instanceof LabError && /unexpectedly skipped/.test(error.message),
  );
});

test("sends the collection token only in the authorization header", async () => {
  const originalFetch = globalThis.fetch;
  let captured;
  globalThis.fetch = async (url, init) => {
    captured = { url, init };
    return new Response(null, { status: 200 });
  };

  try {
    const client = new HttpClient("https://slash.example.com/", "secret-token");
    await client.post("/v1/test-engine/upload", "{}", "application/json");
  } finally {
    globalThis.fetch = originalFetch;
  }

  assert.equal(captured.url, "https://slash.example.com/v1/test-engine/upload");
  assert.equal(captured.init.headers.Authorization, "Bearer secret-token");
  assert.equal(captured.init.headers["Content-Type"], "application/json");
  assert.equal(captured.init.body, "{}");
});
