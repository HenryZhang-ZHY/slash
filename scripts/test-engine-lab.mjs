#!/usr/bin/env node

import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const UPLOAD_PATH = "/v1/test-engine/upload";
const CARGO_PATH = "/v1/test-engine/upload/cargo";
const VITEST_PATH = "/v1/test-engine/upload/vitest";
const QUARANTINE_PATH = "/v1/test-engine/quarantined";

export class LabError extends Error {}

export class HttpClient {
  constructor(baseUrl, token) {
    this.baseUrl = baseUrl.replace(/\/$/, "");
    this.token = token;
  }

  async request(path, init = {}) {
    const response = await fetch(`${this.baseUrl}${path}`, {
      ...init,
      headers: {
        Authorization: `Bearer ${this.token}`,
        ...init.headers,
      },
      signal: AbortSignal.timeout(10_000),
    });
    if (!response.ok) {
      const detail = (await response.text()).trim();
      throw new LabError(
        `${init.method ?? "GET"} ${path} failed with HTTP ${response.status}${
          detail ? `: ${detail}` : ""
        }`,
      );
    }
    return response;
  }

  async post(path, body, contentType) {
    await this.request(path, {
      method: "POST",
      headers: { "Content-Type": contentType },
      body,
    });
  }

  async getJson(path) {
    const response = await this.request(path);
    return response.json();
  }
}

function normalizedPayload(experimentId, attempt, status) {
  return JSON.stringify({
    ci_provider: "local_lab",
    run_ref: `slash-lab/${experimentId}/normalized-${attempt}`,
    executions: [
      {
        name: `slash-lab::${experimentId}::flaky`,
        status,
        duration_ms: 5,
      },
      {
        name: `slash-lab::${experimentId}::steady`,
        status: "passed",
        duration_ms: 3,
      },
    ],
  });
}

function cargoPayload(experimentId) {
  return `${JSON.stringify({
    type: "test",
    name: `slash-lab::${experimentId}::cargo`,
    status: "passed",
    exec_time: 0.004,
  })}\n`;
}

function vitestPayload(experimentId) {
  return JSON.stringify([
    {
      name: `slash-lab::${experimentId}::vitest`,
      status: "passed",
      duration: 6,
      location: { file: "scripts/test-engine-lab.mjs", line: 1 },
    },
  ]);
}

const sleep = (milliseconds) =>
  new Promise((resolve) => setTimeout(resolve, milliseconds));

export async function runExperiment(
  client,
  experimentId,
  { waitSeconds = 70, pollIntervalSeconds = 2, sleeper = sleep } = {},
) {
  const payloads = [
    normalizedPayload(experimentId, 1, "failed"),
    normalizedPayload(experimentId, 2, "passed"),
    normalizedPayload(experimentId, 3, "passed"),
  ];

  for (const payload of payloads) {
    await client.post(UPLOAD_PATH, payload, "application/json");
  }
  await client.post(UPLOAD_PATH, payloads[0], "application/json");
  await client.post(
    CARGO_PATH,
    cargoPayload(experimentId),
    "application/x-ndjson",
  );
  await client.post(
    VITEST_PATH,
    vitestPayload(experimentId),
    "application/json",
  );

  const flakyName = `slash-lab::${experimentId}::flaky`;
  const deadline = Date.now() + waitSeconds * 1_000;
  while (true) {
    const quarantined = await client.getJson(QUARANTINE_PATH);
    const disposition = quarantined.find(({ name }) => name === flakyName);
    if (disposition?.state === "muted") return "muted";
    if (disposition?.state === "skipped") {
      throw new LabError(
        `${flakyName} was unexpectedly skipped; the monitor should use muted`,
      );
    }
    if (Date.now() >= deadline) {
      throw new LabError(
        `${flakyName} was not muted within ${waitSeconds}s; verify the sweeper is running`,
      );
    }
    await sleeper(pollIntervalSeconds * 1_000);
  }
}

function usage() {
  return `Usage: node scripts/test-engine-lab.mjs [options]

Required:
  --base-url URL       Slash server root (or SLASH_TEST_ENGINE_BASE_URL)
  --token-file PATH    File containing the collection token
                       (or SLASH_COLLECTION_TOKEN in the environment)

Options:
  --experiment-id ID   Stable label for this run (default: generated)
  --wait-seconds N     Quarantine wait timeout (default: 70)
  --poll-seconds N     Quarantine poll interval (default: 2)
  --help               Show this help
`;
}

function parseArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const name = argv[index];
    if (name === "--help") return { help: true };
    const value = argv[index + 1];
    if (!value || value.startsWith("--"))
      throw new LabError(`missing value for ${name}`);
    index += 1;
    if (name === "--base-url") options.baseUrl = value;
    else if (name === "--token-file") options.tokenFile = value;
    else if (name === "--experiment-id") options.experimentId = value;
    else if (name === "--wait-seconds") options.waitSeconds = Number(value);
    else if (name === "--poll-seconds")
      options.pollIntervalSeconds = Number(value);
    else throw new LabError(`unknown option: ${name}`);
  }
  return options;
}

function validateOptions(options) {
  const baseUrl = options.baseUrl ?? process.env.SLASH_TEST_ENGINE_BASE_URL;
  if (!baseUrl)
    throw new LabError("set --base-url or SLASH_TEST_ENGINE_BASE_URL");
  const parsedUrl = new URL(baseUrl);
  if (!["http:", "https:"].includes(parsedUrl.protocol)) {
    throw new LabError("base URL must use http or https");
  }

  const token = options.tokenFile
    ? readFileSync(options.tokenFile, "utf8").trim()
    : process.env.SLASH_COLLECTION_TOKEN?.trim();
  if (!token) throw new LabError("set --token-file or SLASH_COLLECTION_TOKEN");

  const experimentId =
    options.experimentId ??
    `${new Date().toISOString().replace(/[-:.TZ]/g, "")}-${randomUUID().slice(0, 8)}`;
  if (!/^[A-Za-z0-9._-]+$/.test(experimentId)) {
    throw new LabError(
      "experiment id may contain only letters, digits, dot, underscore, and dash",
    );
  }

  const waitSeconds = options.waitSeconds ?? 70;
  const pollIntervalSeconds = options.pollIntervalSeconds ?? 2;
  if (!Number.isFinite(waitSeconds) || waitSeconds < 0) {
    throw new LabError("wait seconds must be a non-negative number");
  }
  if (!Number.isFinite(pollIntervalSeconds) || pollIntervalSeconds <= 0) {
    throw new LabError("poll seconds must be a positive number");
  }
  return {
    baseUrl: parsedUrl.toString(),
    token,
    experimentId,
    waitSeconds,
    pollIntervalSeconds,
  };
}

async function main() {
  const parsed = parseArgs(process.argv.slice(2));
  if (parsed.help) {
    process.stdout.write(usage());
    return;
  }
  const options = validateOptions(parsed);
  const state = await runExperiment(
    new HttpClient(options.baseUrl, options.token),
    options.experimentId,
    {
      waitSeconds: options.waitSeconds,
      pollIntervalSeconds: options.pollIntervalSeconds,
    },
  );
  process.stdout.write(
    `Local Test Engine experiment ${options.experimentId} passed: replay was accepted, collector dialects were accepted, and the flaky test became ${state}.\n`,
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main().catch((error) => {
    process.stderr.write(`Test Engine lab failed: ${error.message}\n`);
    process.exitCode = 1;
  });
}
