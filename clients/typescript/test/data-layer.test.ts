import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, test } from "node:test";
import assert from "node:assert/strict";

import { DataLayer, DataLayerConnectionError, DataLayerRequestError } from "../src/index";
import type { AnonymizeResult } from "../src/index";

const FIXTURES = join(__dirname, "..", "..", "test", "fixtures");

function fixture(name: string): string {
  return readFileSync(join(FIXTURES, name), "utf8");
}

function jsonResponse(body: string, init: { status?: number; contentType?: string } = {}): Response {
  return new Response(body, {
    status: init.status ?? 200,
    headers: { "content-type": init.contentType ?? "application/json" },
  });
}

const originalFetch = global.fetch;

afterEach(() => {
  global.fetch = originalFetch;
});

test("connect resolves once /health answers", async () => {
  global.fetch = (async () => jsonResponse(fixture("health-response.json"))) as typeof fetch;

  const dataLayer = await DataLayer.connect("http://data-layer:8080");
  assert.ok(dataLayer instanceof DataLayer);
});

test("connect rejects when /health is unreachable", async () => {
  global.fetch = (async () => {
    throw new Error("ECONNREFUSED");
  }) as typeof fetch;

  await assert.rejects(
    () => DataLayer.connect("http://nowhere:8080"),
    DataLayerConnectionError,
  );
});

test("connect rejects when /health responds with an error status", async () => {
  global.fetch = (async () => jsonResponse("{}", { status: 503 })) as typeof fetch;

  await assert.rejects(
    () => DataLayer.connect("http://data-layer:8080"),
    DataLayerConnectionError,
  );
});

test("connect rejects when /health answers 2xx but reports a status other than ok", async () => {
  global.fetch = (async () =>
    jsonResponse(JSON.stringify({ status: "degraded", version: "0.1.0" }))) as typeof fetch;

  // The misconfiguration this factory exists to catch: a URL pointing at an
  // unrelated service that happens to answer `/health` with a 2xx and a
  // JSON body of its own.
  await assert.rejects(
    () => DataLayer.connect("http://wrong-service:8080"),
    DataLayerConnectionError,
  );
});

test("anonymize parses a recorded success response", async () => {
  const healthBody = fixture("health-response.json");
  const successBody = fixture("anonymize-success-response.json");

  global.fetch = (async (input: RequestInfo | URL) => {
    const url = String(input);
    return url.endsWith("/health") ? jsonResponse(healthBody) : jsonResponse(successBody);
  }) as typeof fetch;

  const dataLayer = await DataLayer.connect("http://data-layer:8080");
  const result: AnonymizeResult = await dataLayer.anonymize({
    dataset: { tables: [] },
    options: { seed: 1 },
  });

  assert.equal(result.success, true);
  if (!result.success) {
    throw new Error("expected success");
  }

  assert.equal(result.dataset.tables[0]?.name, "customers");
  assert.equal(result.report.verdict, "pass");
  assert.equal(result.report.checks[0]?.check, "leakage");
  assert.equal(result.report.accepted_strength, "default");

  const emailColumn = result.report.columns.find((column) => column.column === "customers.email");
  assert.equal(emailColumn?.semantic_type, "email");
  assert.equal(emailColumn?.strategy, "pseudonymize_local_part");

  const rows = result.dataset.tables[0]?.rows ?? [];
  assert.equal(rows.length, 6);
  for (const row of rows) {
    const email = row[1];
    assert.equal(typeof email, "string");
    assert.ok(!String(email).startsWith("ana@"));
  }
});

test("anonymize surfaces a recorded problem+json response without throwing", async () => {
  const healthBody = fixture("health-response.json");
  const problemBody = fixture("anonymize-problem-response.json");

  global.fetch = (async (input: RequestInfo | URL) => {
    const url = String(input);
    return url.endsWith("/health")
      ? jsonResponse(healthBody)
      : jsonResponse(problemBody, { status: 422, contentType: "application/problem+json" });
  }) as typeof fetch;

  const dataLayer = await DataLayer.connect("http://data-layer:8080");
  const result = await dataLayer.anonymize({ dataset: { tables: [] } });

  assert.equal(result.success, false);
  if (result.success) {
    throw new Error("expected failure");
  }
  assert.equal(result.error.status, 422);
  assert.equal(result.error.type, "urn:placeholder:data-layer:invalid-dataset");
  assert.match(result.error.detail ?? "", /row 1 has 1 values/);
});

test("anonymize preserves three keys that only differ past the 53rd bit", async () => {
  const healthBody = fixture("health-response.json");
  // Three distinct minted keys that a plain `JSON.parse` collapses onto the
  // same `5959995937546830000` once they round to the nearest double. The
  // ledger's injectivity guarantee does not survive that, which is exactly
  // the defect this client has to not reintroduce. Written as a raw string,
  // never as a JS numeric literal: writing `5959995937546829429` directly
  // in source would already be rounded by the time this file is parsed,
  // before any code under test ever runs.
  const successBody =
    '{"run_id":"11111111-1111-1111-1111-111111111111",' +
    '"dataset":{"tables":[{"name":"customers",' +
    '"columns":[{"name":"id","type":"integer","nullable":false,"pk":true}],' +
    '"rows":[[5959995937546829429],[5959995937546829430],[5959995937546829431]]}]},' +
    '"report":{"verdict":"pass","columns":[],"checks":[],"preserved_columns":{},' +
    '"timings_ms":{},"accepted_strength":"default","attempts":1}}';

  global.fetch = (async (input: RequestInfo | URL) => {
    const url = String(input);
    return url.endsWith("/health") ? jsonResponse(healthBody) : jsonResponse(successBody);
  }) as typeof fetch;

  const dataLayer = await DataLayer.connect("http://data-layer:8080");
  const result = await dataLayer.anonymize({ dataset: { tables: [] } });

  assert.equal(result.success, true);
  if (!result.success) {
    throw new Error("expected success");
  }

  const keys = result.dataset.tables[0]?.rows.map((row) => row[0]) ?? [];
  assert.deepEqual(keys, ["5959995937546829429", "5959995937546829430", "5959995937546829431"]);
  assert.equal(new Set(keys).size, 3, "all three keys must remain distinguishable");
});

test("profile parses a recorded response", async () => {
  const healthBody = fixture("health-response.json");
  const profileBody = JSON.stringify({
    run_id: "22222222-2222-2222-2222-222222222222",
    profile: {
      columns: {
        "customers.email": {
          column: "customers.email",
          semantic_type: "email",
          confidence: 1.0,
          null_rate: 0.0,
          distinct: 2,
          length: null,
          distribution: { kind: "categorical", top: { "gmail.com": 1.0 }, other: 0.0, cardinality: 1 },
          anomaly_rates: {},
        },
      },
    },
  });

  global.fetch = (async (input: RequestInfo | URL) => {
    const url = String(input);
    return url.endsWith("/health") ? jsonResponse(healthBody) : jsonResponse(profileBody);
  }) as typeof fetch;

  const dataLayer = await DataLayer.connect("http://data-layer:8080");
  const result = await dataLayer.profile({ dataset: { tables: [] } });

  assert.equal(result.success, true);
  if (!result.success) {
    throw new Error("expected success");
  }
  const emailProfile = result.profile.columns["customers.email"];
  assert.equal(emailProfile?.semantic_type, "email");
  assert.equal(emailProfile?.distinct, 2);
});

test("detectors returns the semantic types this build recognizes", async () => {
  const healthBody = fixture("health-response.json");
  global.fetch = (async (input: RequestInfo | URL) => {
    const url = String(input);
    return url.endsWith("/health")
      ? jsonResponse(healthBody)
      : jsonResponse(JSON.stringify(["email", "uuid", "cpf"]));
  }) as typeof fetch;

  const dataLayer = await DataLayer.connect("http://data-layer:8080");
  const detectors = await dataLayer.detectors();

  assert.deepEqual(detectors, ["email", "uuid", "cpf"]);
});

test("jobs returns in-flight jobs without dataset contents", async () => {
  const healthBody = fixture("health-response.json");
  const jobsBody = JSON.stringify([
    { id: "3fa85f64-5717-4562-b3fc-2c963f66afa6", kind: "anonymize", state: "running", rows: 42 },
  ]);
  global.fetch = (async (input: RequestInfo | URL) => {
    const url = String(input);
    return url.endsWith("/health") ? jsonResponse(healthBody) : jsonResponse(jobsBody);
  }) as typeof fetch;

  const dataLayer = await DataLayer.connect("http://data-layer:8080");
  const jobs = await dataLayer.jobs();

  assert.equal(jobs.length, 1);
  assert.equal(jobs[0]?.kind, "anonymize");
  assert.equal(jobs[0]?.state, "running");
});

test("jobs wraps a transport failure in DataLayerRequestError", async () => {
  const healthBody = fixture("health-response.json");
  let callCount = 0;
  global.fetch = (async (input: RequestInfo | URL) => {
    const url = String(input);
    if (url.endsWith("/health")) {
      return jsonResponse(healthBody);
    }
    callCount += 1;
    throw new Error("ECONNRESET");
  }) as typeof fetch;

  const dataLayer = await DataLayer.connect("http://data-layer:8080");
  await assert.rejects(() => dataLayer.jobs(), DataLayerRequestError);
  assert.equal(callCount, 1);
});

test("a non-2xx response without a problem+json body still throws DataLayerRequestError", async () => {
  const healthBody = fixture("health-response.json");
  global.fetch = (async (input: RequestInfo | URL) => {
    const url = String(input);
    return url.endsWith("/health")
      ? jsonResponse(healthBody)
      : jsonResponse("upstream is on fire", { status: 500, contentType: "text/plain" });
  }) as typeof fetch;

  const dataLayer = await DataLayer.connect("http://data-layer:8080");
  await assert.rejects(() => dataLayer.jobs(), DataLayerRequestError);

  const anonymizeResult = dataLayer.anonymize({ dataset: { tables: [] } });
  await assert.rejects(() => anonymizeResult, DataLayerRequestError);
});
